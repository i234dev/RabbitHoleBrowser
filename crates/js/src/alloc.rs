//! A small-object memory allocator for the threads that run JavaScript.
//!
//! Script execution allocates and frees enormous numbers of small blocks
//! (strings, property vectors, frames, AST nodes); in the system allocator
//! that was a fifth of a page load. [`RhAlloc`] serves blocks of up to
//! [`MAX_SMALL`] bytes from per-thread free lists of fixed size classes,
//! carved from 256 KiB chunks, on threads that opted in
//! ([`enable_for_this_thread`], done by every new [`crate::Realm`]); every
//! other request goes to the system allocator.
//!
//! * A block's size class follows from the `Layout` passed to `dealloc`, so
//!   blocks carry no header.
//! * Chunks are registered in a lock-free table, so `dealloc` can tell its
//!   own blocks from system ones on any thread.
//! * A thread frees into its own lists (blocks of one class are
//!   interchangeable). Threads that did not opt in hand freed blocks to a
//!   global pool, which opted-in threads drain before carving new memory.
//! * Chunk memory is reused, never returned to the operating system.
//!
//! Install it in a binary with
//! `#[global_allocator] static A: rabbithole_js::alloc::RhAlloc = rabbithole_js::alloc::RhAlloc;`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::UnsafeCell;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// Largest request served from size classes.
pub const MAX_SMALL: usize = 1024;
const CHUNK: usize = 1 << 18;
/// 16-byte granularity up to 256 bytes, then 64-byte steps.
const NCLASS: usize = 16 + 12;
const TABLE_BITS: u32 = 16;

#[inline(always)]
fn class_of(size: usize) -> usize {
    if size <= 256 {
        (size.max(1) + 15) / 16 - 1
    } else {
        16 + (size - 256 + 63) / 64 - 1
    }
}

#[inline(always)]
fn class_size(c: usize) -> usize {
    if c < 16 {
        (c + 1) * 16
    } else {
        256 + (c - 15) * 64
    }
}

/// Registered chunk bases (open addressing, insert-only).
static CHUNKS: [AtomicUsize; 1 << TABLE_BITS] = [const { AtomicUsize::new(0) }; 1 << TABLE_BITS];
static NCHUNKS: AtomicUsize = AtomicUsize::new(0);

#[inline(always)]
fn slot_of(base: usize) -> usize {
    ((base >> 18).wrapping_mul(0x9E37_79B9_7F4A_7C15usize) >> (usize::BITS - TABLE_BITS)) & ((1 << TABLE_BITS) - 1)
}

#[inline]
fn is_ours(p: *mut u8) -> bool {
    if NCHUNKS.load(Ordering::Relaxed) == 0 {
        return false;
    }
    let base = p as usize & !(CHUNK - 1);
    let mut i = slot_of(base);
    loop {
        let e = CHUNKS[i].load(Ordering::Acquire);
        if e == base {
            return true;
        }
        if e == 0 {
            return false;
        }
        i = (i + 1) & ((1 << TABLE_BITS) - 1);
    }
}

fn register(base: usize) -> bool {
    // Keep the table at most half full.
    if NCHUNKS.load(Ordering::Relaxed) >= (1 << TABLE_BITS) / 2 {
        return false;
    }
    let mut i = slot_of(base);
    loop {
        match CHUNKS[i].compare_exchange(0, base, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => {
                NCHUNKS.fetch_add(1, Ordering::Relaxed);
                return true;
            }
            Err(_) => i = (i + 1) & ((1 << TABLE_BITS) - 1),
        }
    }
}

/// Free blocks shared by all threads: per class, a list linked through
/// the blocks' first word, plus the chunk being carved.
struct Pool {
    lists: [usize; NCLASS],
    carve: usize,
    carve_end: usize,
}

static POOL: Mutex<Pool> = Mutex::new(Pool { lists: [0; NCLASS], carve: 0, carve_end: 0 });

struct Cache {
    enabled: bool,
    lists: [*mut u8; NCLASS],
}

thread_local! {
    static CACHE: UnsafeCell<Cache> = const { UnsafeCell::new(Cache { enabled: false, lists: [null_mut(); NCLASS] }) };
}

/// Serve this thread's small allocations from the size-class allocator
/// (when it is the global allocator). Meant for long-lived threads that
/// allocate heavily (script and layout threads).
pub fn enable_for_this_thread() {
    // SAFETY: the cache is only touched by its own thread.
    CACHE.with(|c| unsafe { (*c.get()).enabled = true });
}

#[inline(always)]
unsafe fn next_of(p: *mut u8) -> *mut u8 {
    *(p as *mut *mut u8)
}

#[inline(always)]
unsafe fn set_next(p: *mut u8, n: *mut u8) {
    *(p as *mut *mut u8) = n;
}

/// Fill `c`'s list for class `k` (empty) from the pool or new memory.
#[cold]
unsafe fn refill(c: &mut Cache, k: usize) -> bool {
    let size = class_size(k);
    let Ok(mut pool) = POOL.lock() else { return false };
    let shared = pool.lists[k];
    if shared != 0 {
        // Take the whole shared list.
        pool.lists[k] = 0;
        c.lists[k] = shared as *mut u8;
        return true;
    }
    // Carve a batch of about 8 KiB.
    let want = (8192 / size).max(4);
    let mut head: *mut u8 = null_mut();
    let mut n = 0usize;
    for _ in 0..want {
        if pool.carve + size > pool.carve_end {
            let chunk = System.alloc(Layout::from_size_align_unchecked(CHUNK, CHUNK));
            if chunk.is_null() || !register(chunk as usize) {
                if !chunk.is_null() {
                    System.dealloc(chunk, Layout::from_size_align_unchecked(CHUNK, CHUNK));
                }
                break;
            }
            pool.carve = chunk as usize;
            pool.carve_end = chunk as usize + CHUNK;
        }
        let b = pool.carve as *mut u8;
        pool.carve += size;
        set_next(b, head);
        head = b;
        n += 1;
    }
    if n == 0 {
        return false;
    }
    c.lists[k] = head;
    true
}

unsafe fn free_to_pool(p: *mut u8, k: usize) {
    if let Ok(mut pool) = POOL.lock() {
        set_next(p, pool.lists[k] as *mut u8);
        pool.lists[k] = p as usize;
    }
}

/// The allocator (see the module docs).
pub struct RhAlloc;

unsafe impl GlobalAlloc for RhAlloc {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.size() <= MAX_SMALL && layout.align() <= 16 {
            let k = class_of(layout.size());
            let r = CACHE.try_with(|c| {
                let c = &mut *c.get();
                if !c.enabled {
                    return null_mut();
                }
                if c.lists[k].is_null() && !refill(c, k) {
                    return null_mut();
                }
                let p = c.lists[k];
                c.lists[k] = next_of(p);
                p
            });
            if let Ok(p) = r {
                if !p.is_null() {
                    return p;
                }
            }
        }
        System.alloc(layout)
    }

    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        if layout.size() <= MAX_SMALL && layout.align() <= 16 && is_ours(p) {
            let k = class_of(layout.size());
            let done = CACHE.try_with(|c| {
                let c = &mut *c.get();
                if !c.enabled {
                    return false;
                }
                set_next(p, c.lists[k]);
                c.lists[k] = p;
                true
            });
            if done != Ok(true) {
                free_to_pool(p, k);
            }
            return;
        }
        System.dealloc(p, layout)
    }

    #[inline]
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if layout.size() <= MAX_SMALL && layout.align() <= 16 && is_ours(p) {
            if new_size <= MAX_SMALL && class_of(new_size) == class_of(layout.size()) {
                return p;
            }
            let q = self.alloc(Layout::from_size_align_unchecked(new_size, layout.align()));
            if !q.is_null() {
                std::ptr::copy_nonoverlapping(p, q, layout.size().min(new_size));
                self.dealloc(p, layout);
            }
            return q;
        }
        // A system block stays one (`dealloc` tells them apart by address).
        System.realloc(p, layout, new_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_cover_sizes() {
        for s in 1..=MAX_SMALL {
            let k = class_of(s);
            assert!(k < NCLASS, "size {s}");
            assert!(class_size(k) >= s, "size {s} class {k}");
            if k > 0 {
                assert!(class_size(k - 1) < s, "size {s} not in the smallest class");
            }
            assert_eq!(class_size(k) % 16, 0);
        }
    }

    #[test]
    fn blocks_round_trip_across_threads() {
        let a = RhAlloc;
        enable_for_this_thread();
        unsafe {
            let mut v = Vec::new();
            for i in 0..5000usize {
                let l = Layout::from_size_align(1 + i % 700, 8).unwrap();
                let p = a.alloc(l);
                assert!(!p.is_null());
                std::ptr::write_bytes(p, (i % 251) as u8, l.size());
                v.push((p as usize, l));
            }
            for (i, (p, l)) in v.iter().enumerate() {
                assert_eq!(*(*p as *const u8) as usize, i % 251);
                assert_eq!(*((*p + l.size() - 1) as *const u8) as usize, i % 251);
            }
            // Free half on another (not opted-in) thread, half here.
            let (here, there): (Vec<_>, Vec<_>) = v.into_iter().enumerate().partition(|(i, _)| i % 2 == 0);
            std::thread::spawn(move || {
                for (_, (p, l)) in there {
                    RhAlloc.dealloc(p as *mut u8, l);
                }
            })
            .join()
            .unwrap();
            for (_, (p, l)) in here {
                a.dealloc(p as *mut u8, l);
            }
            // Reuse and grow.
            let l = Layout::from_size_align(24, 8).unwrap();
            let p = a.alloc(l);
            std::ptr::write_bytes(p, 7, 24);
            let q = a.realloc(p, l, 2000);
            assert_eq!(*q.add(23), 7);
            a.dealloc(q, Layout::from_size_align(2000, 8).unwrap());
        }
    }
}
