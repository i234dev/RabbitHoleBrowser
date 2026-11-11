//! Native stack guard.
//!
//! The interpreter runs JS→JS calls without Rust recursion, but every
//! re-entry from Rust (native functions calling back into JS, getters,
//! Proxy traps, `valueOf`/`toString`, constructors…) nests another
//! interpreter loop on the native stack. Instead of a fixed depth limit
//! (too low for legitimate code, or too high for small thread stacks),
//! like V8 we compare the stack pointer with the real bounds of the
//! current thread's stack and throw `RangeError: Maximum call stack size
//! exceeded` while a safety reserve is still free.

use std::cell::Cell;

thread_local! {
    /// Lowest stack address JS may use on this thread (0 = not computed).
    static LIMIT: Cell<usize> = const { Cell::new(0) };
}

/// Approximate current stack pointer.
#[inline(always)]
pub fn current_sp() -> usize {
    let marker = 0u8;
    std::hint::black_box(&marker) as *const u8 as usize
}

/// Bounds `(low, high)` of the current thread's stack, if the platform
/// can tell.
#[cfg(all(target_os = "linux", any(target_env = "gnu", target_env = "musl")))]
pub fn thread_stack_bounds() -> Option<(usize, usize)> {
    // pthread_attr_t is 56 (x86_64) / 64 (aarch64) bytes; over-allocate.
    #[repr(C, align(16))]
    struct Attr([u8; 256]);
    extern "C" {
        fn pthread_self() -> usize;
        fn pthread_getattr_np(thread: usize, attr: *mut Attr) -> i32;
        fn pthread_attr_getstack(attr: *const Attr, addr: *mut *mut u8, size: *mut usize) -> i32;
        fn pthread_attr_destroy(attr: *mut Attr) -> i32;
    }
    let mut attr = Attr([0; 256]);
    // SAFETY: plain libc calls on a properly sized, initialised buffer.
    unsafe {
        if pthread_getattr_np(pthread_self(), &mut attr) != 0 {
            return None;
        }
        let mut addr: *mut u8 = std::ptr::null_mut();
        let mut size = 0usize;
        let ok = pthread_attr_getstack(&attr, &mut addr, &mut size) == 0;
        pthread_attr_destroy(&mut attr);
        if !ok || addr.is_null() || size == 0 {
            return None;
        }
        Some((addr as usize, addr as usize + size))
    }
}

#[cfg(target_os = "windows")]
pub fn thread_stack_bounds() -> Option<(usize, usize)> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThreadStackLimits(low: *mut usize, high: *mut usize);
    }
    let (mut low, mut high) = (0usize, 0usize);
    // SAFETY: Win32 call writing two out-parameters.
    unsafe { GetCurrentThreadStackLimits(&mut low, &mut high) };
    (low != 0 && high > low).then_some((low, high))
}

#[cfg(target_os = "macos")]
pub fn thread_stack_bounds() -> Option<(usize, usize)> {
    extern "C" {
        fn pthread_self() -> usize;
        fn pthread_get_stackaddr_np(thread: usize) -> *mut u8;
        fn pthread_get_stacksize_np(thread: usize) -> usize;
    }
    // SAFETY: plain libc calls about the current thread.
    unsafe {
        let t = pthread_self();
        let high = pthread_get_stackaddr_np(t) as usize;
        let size = pthread_get_stacksize_np(t);
        (high > size && size > 0).then(|| (high - size, high))
    }
}

#[cfg(not(any(all(target_os = "linux", any(target_env = "gnu", target_env = "musl")), target_os = "windows", target_os = "macos")))]
pub fn thread_stack_bounds() -> Option<(usize, usize)> {
    None
}

/// Stack kept free below the limit for native work started from JS
/// (layout, regex, JSON, the parser…).
fn reserve(size: usize) -> usize {
    (size / 8).clamp(256 << 10, 8 << 20)
}

#[cold]
fn compute_limit() -> usize {
    let sp = current_sp();
    let limit = match thread_stack_bounds() {
        Some((low, high)) if sp > low && sp <= high => low + reserve(high - low),
        // Unknown platform: assume about 1 MiB below the first JS entry.
        _ => sp.saturating_sub(1 << 20),
    };
    limit.max(1)
}

/// The lowest stack address JS may reach on this thread.
#[inline]
pub fn limit() -> usize {
    LIMIT.with(|l| {
        let v = l.get();
        if v != 0 {
            v
        } else {
            let v = compute_limit();
            l.set(v);
            v
        }
    })
}

/// Is the native stack nearly exhausted?
#[inline]
pub fn exhausted() -> bool {
    current_sp() < limit()
}

/// Bytes of native stack still usable by JS on this thread.
pub fn remaining() -> usize {
    current_sp().saturating_sub(limit())
}

#[cfg(test)]
mod tests {
    #[test]
    fn bounds_contain_the_current_frame() {
        if let Some((low, high)) = super::thread_stack_bounds() {
            let sp = super::current_sp();
            assert!(low < sp && sp <= high, "{low:x} < {sp:x} <= {high:x}");
        }
        assert!(!super::exhausted());
        assert!(super::remaining() > 0);
    }
}
