//! Shared infrastructure of RabbitHole's baseline (template) compilers:
//!
//! * [`ExecMem`]: executable memory with W^X — mapped read-write, filled,
//!   then switched to read-execute; never writable and executable at once.
//!   Freed on drop.
//! * [`x64::Asm`]: a small x86-64 assembler (the instructions templates
//!   need, labels with rel32 fixups, and an optional listing with offsets
//!   and mnemonics for debug dumps; there is no disassembler here).
//!
//! The crate has no dependencies and knows nothing about JavaScript or
//! WebAssembly: each engine owns its templates, helpers and calling
//! convention (see docs/jit-design.md).

pub mod x64;

/// A mapping of native code, executable and not writable. Unmapped on drop.
pub struct ExecMem {
    ptr: *mut u8,
    len: usize,
    mapped_len: usize,
}

#[cfg(test)]
thread_local! {
    // Lifecycle accounting also covers unwinding from the patch callback.
    static LIVE_MAPPED_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl std::fmt::Debug for ExecMem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ExecMem({:p}, {} bytes)", self.ptr, self.len)
    }
}

impl ExecMem {
    /// Map `code`, apply `patch` to the writable copy (e.g. absolute
    /// addresses that depend on where it landed), then make it
    /// read-execute. `None` if the platform refuses or is unsupported.
    pub fn new(code: &[u8], patch: impl FnOnce(*mut u8, &mut [u8])) -> Option<ExecMem> {
        let len = code.len().max(1);
        let mapped_len = Self::mapped_len_for(code.len())?;
        // SAFETY: a fresh private mapping of `len` bytes, written while
        // read-write and switched to read-execute before any use.
        unsafe {
            let p = sys::alloc(mapped_len)?;
            // Install the owner before invoking caller code: a panicking
            // patch must release the still-writable mapping as well.
            let mem = ExecMem { ptr: p, len, mapped_len };
            #[cfg(test)]
            LIVE_MAPPED_BYTES.with(|n| n.set(n.get() + mapped_len));
            std::ptr::copy_nonoverlapping(code.as_ptr(), p, code.len());
            patch(p, std::slice::from_raw_parts_mut(p, code.len()));
            if !sys::protect(p, mapped_len) {
                return None;
            }
            Some(mem)
        }
    }

    pub fn ptr(&self) -> *const u8 {
        self.ptr
    }

    /// Bytes requested (the mapping is rounded up to whole pages).
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Page-rounded mapping bytes (committed pages on Windows). This is
    /// neither physical RSS nor Windows virtual-address reservation size.
    pub fn mapped_len(&self) -> usize {
        self.mapped_len
    }

    /// Mapping bytes required for code of this length, before allocating.
    /// Uses the host's protection/commitment page size, not its address
    /// allocation granularity. Empty code still receives one page.
    pub fn mapped_len_for(code_len: usize) -> Option<usize> {
        static PAGE: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
        let page = (*PAGE.get_or_init(sys::page_size))?;
        code_len.max(1).checked_add(page - 1)?.checked_div(page)?.checked_mul(page)
    }

    pub fn contains(&self, addr: usize) -> bool {
        addr >= self.ptr as usize && addr < self.ptr as usize + self.len
    }
}

impl Drop for ExecMem {
    fn drop(&mut self) {
        // SAFETY: allocated by `sys::alloc` with this length.
        unsafe { sys::free(self.ptr, self.mapped_len) }
        #[cfg(test)]
        LIVE_MAPPED_BYTES.with(|n| n.set(n.get() - self.mapped_len));
    }
}

/// Is native code generation possible on this target (x86-64 Linux,
/// macOS without hardened runtime, Windows)?
pub const fn supported() -> bool {
    cfg!(all(target_arch = "x86_64", any(unix, windows)))
}

/// Linux: does the process have any writable and executable mapping?
/// (Tests use it to check W^X.) Elsewhere: `None`.
pub fn has_rwx_mapping() -> Option<bool> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
    Some(maps.lines().any(|l| l.split_whitespace().nth(1).map(|p| p.starts_with("rwx")).unwrap_or(false)))
}

#[cfg(unix)]
mod sys {
    extern "C" {
        fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
        fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32;
        fn munmap(addr: *mut u8, len: usize) -> i32;
        fn getpagesize() -> i32;
    }
    const PROT_READ: i32 = 1;
    const PROT_WRITE: i32 = 2;
    const PROT_EXEC: i32 = 4;
    const MAP_PRIVATE: i32 = 2;
    #[cfg(target_os = "macos")]
    const MAP_ANON: i32 = 0x1000;
    #[cfg(not(target_os = "macos"))]
    const MAP_ANON: i32 = 0x20;
    pub fn page_size() -> Option<usize> {
        // SAFETY: getpagesize takes no arguments and returns process-wide
        // memory-management configuration.
        usize::try_from(unsafe { getpagesize() }).ok().filter(|n| *n != 0)
    }
    pub unsafe fn alloc(len: usize) -> Option<*mut u8> {
        let p = mmap(std::ptr::null_mut(), len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
        if p.is_null() || p as isize == -1 {
            return None;
        }
        Some(p)
    }
    pub unsafe fn protect(p: *mut u8, len: usize) -> bool {
        mprotect(p, len, PROT_READ | PROT_EXEC) == 0
    }
    pub unsafe fn free(p: *mut u8, len: usize) {
        munmap(p, len);
    }
}

#[cfg(windows)]
mod sys {
    #[repr(C)]
    struct SystemInfo {
        processor_architecture: u32,
        page_size: u32,
        minimum_address: *mut u8,
        maximum_address: *mut u8,
        active_processor_mask: usize,
        processor_count: u32,
        processor_type: u32,
        allocation_granularity: u32,
        processor_level: u16,
        processor_revision: u16,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetSystemInfo(info: *mut SystemInfo);
        fn VirtualAlloc(addr: *mut u8, size: usize, ty: u32, protect: u32) -> *mut u8;
        fn VirtualProtect(addr: *mut u8, size: usize, protect: u32, old: *mut u32) -> i32;
        fn VirtualFree(addr: *mut u8, size: usize, ty: u32) -> i32;
        fn FlushInstructionCache(process: isize, addr: *const u8, size: usize) -> i32;
        fn GetCurrentProcess() -> isize;
    }
    const MEM_COMMIT: u32 = 0x1000;
    const MEM_RESERVE: u32 = 0x2000;
    const MEM_RELEASE: u32 = 0x8000;
    const PAGE_READWRITE: u32 = 0x04;
    const PAGE_EXECUTE_READ: u32 = 0x20;
    pub fn page_size() -> Option<usize> {
        let mut info = std::mem::MaybeUninit::<SystemInfo>::zeroed();
        // SAFETY: the repr(C) layout matches SYSTEM_INFO and the API
        // initializes the provided writable structure before returning.
        let size = unsafe {
            GetSystemInfo(info.as_mut_ptr());
            info.assume_init().page_size as usize
        };
        (size != 0).then_some(size)
    }
    pub unsafe fn alloc(len: usize) -> Option<*mut u8> {
        let p = VirtualAlloc(std::ptr::null_mut(), len, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
        if p.is_null() {
            None
        } else {
            Some(p)
        }
    }
    pub unsafe fn protect(p: *mut u8, len: usize) -> bool {
        let mut old = 0;
        let ok = VirtualProtect(p, len, PAGE_EXECUTE_READ, &mut old) != 0;
        FlushInstructionCache(GetCurrentProcess(), p, len);
        ok
    }
    pub unsafe fn free(p: *mut u8, _len: usize) {
        VirtualFree(p, 0, MEM_RELEASE);
    }
}

#[cfg(not(any(unix, windows)))]
mod sys {
    pub fn page_size() -> Option<usize> {
        None
    }
    pub unsafe fn alloc(_len: usize) -> Option<*mut u8> {
        None
    }
    pub unsafe fn protect(_p: *mut u8, _len: usize) -> bool {
        false
    }
    pub unsafe fn free(_p: *mut u8, _len: usize) {}
}

#[cfg(test)]
mod mapping_tests {
    use super::*;

    #[test]
    fn mapping_size_uses_host_pages_and_rejects_overflow() {
        let Some(page) = sys::page_size() else { return };
        for size in [0, 1, page - 1, page] {
            assert_eq!(ExecMem::mapped_len_for(size), Some(page));
        }
        assert_eq!(ExecMem::mapped_len_for(page + 1), Some(2 * page));
        assert!(ExecMem::mapped_len_for(usize::MAX).is_none());
        if supported() {
            let mem = ExecMem::new(&vec![0; page + 1], |_, _| {}).expect("map");
            assert_eq!(mem.len(), page + 1);
            assert_eq!(mem.mapped_len(), 2 * page);
        }
    }

    #[test]
    fn patch_panic_releases_writable_mapping() {
        if !supported() { return; }
        let before = LIVE_MAPPED_BYTES.with(|n| n.get());
        let panic = std::panic::catch_unwind(|| {
            ExecMem::new(&[0; 32], |_, bytes| {
                assert!(LIVE_MAPPED_BYTES.with(|n| n.get()) > before);
                bytes.fill(0xC3);
                panic!("patch failed");
            });
        });
        assert!(panic.is_err());
        assert_eq!(LIVE_MAPPED_BYTES.with(|n| n.get()), before);
    }
}

#[cfg(all(test, target_arch = "x86_64"))]
mod tests {
    use super::x64::*;
    use super::*;

    #[test]
    fn stub_runs_and_no_rwx() {
        if !supported() {
            return;
        }
        // mov eax, 0 ; ret  — then a function returning its argument + 7.
        let mut a = Asm::new(true);
        a.mov_imm32(RAX, 0);
        a.ret();
        let m = ExecMem::new(&a.buf, |_, _| {}).expect("map");
        let f: extern "sysv64" fn() -> u32 = unsafe { std::mem::transmute(m.ptr()) };
        assert_eq!(f(), 0);
        let mut a = Asm::new(false);
        a.mov_rr64(RAX, RDI);
        a.add_ri64(RAX, 7);
        a.ret();
        let m2 = ExecMem::new(&a.buf, |_, _| {}).expect("map");
        let g: extern "sysv64" fn(u64) -> u64 = unsafe { std::mem::transmute(m2.ptr()) };
        assert_eq!(g(35), 42);
        if let Some(rwx) = has_rwx_mapping() {
            assert!(!rwx, "a writable+executable mapping exists");
        }
        drop(m);
        drop(m2);
    }

    #[test]
    fn labels_and_loops() {
        if !supported() {
            return;
        }
        // sum = 0; for i in 0..n { sum += i }  (n in rdi)
        let mut a = Asm::new(true);
        let (top, done) = (a.label(), a.label());
        a.xor_rr32(RAX, RAX);
        a.xor_rr32(RCX, RCX);
        a.bind(top);
        a.cmp_rr64(RCX, RDI);
        a.jcc(CC_AE, done);
        a.add_rr64(RAX, RCX);
        a.add_ri64(RCX, 1);
        a.jmp(top);
        a.bind(done);
        a.ret();
        a.finish_labels();
        assert!(a.listing().unwrap().contains("add rax, rcx"));
        let m = ExecMem::new(&a.buf, |_, _| {}).expect("map");
        let f: extern "sysv64" fn(u64) -> u64 = unsafe { std::mem::transmute(m.ptr()) };
        assert_eq!(f(10), 45);
        assert_eq!(f(0), 0);
    }

    #[test]
    fn sse_and_memory() {
        if !supported() {
            return;
        }
        // [rdi] = [rsi] + [rsi+8] as f64
        let mut a = Asm::new(false);
        a.movsd_load(XMM0, Mem::base(RSI, 0));
        a.movsd_load(XMM1, Mem::base(RSI, 8));
        a.sse_rr(SSE_ADD, XMM0, XMM1);
        a.movq_r_x(RAX, XMM0);
        a.store64(Mem::base(RDI, 0), RAX);
        a.ret();
        let m = ExecMem::new(&a.buf, |_, _| {}).expect("map");
        let f: extern "sysv64" fn(*mut f64, *const f64) = unsafe { std::mem::transmute(m.ptr()) };
        let mut out = 0.0;
        let inp = [1.5, 2.25];
        f(&mut out, inp.as_ptr());
        assert_eq!(out, 3.75);
    }
}

#[cfg(all(test, not(target_arch = "x86_64")))]
#[test]
fn native_backend_is_unavailable() {
    assert!(!supported());
}
