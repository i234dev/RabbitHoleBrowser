//! Native WebAssembly execution on x86-64, with interpreter fallback on
//! architectures without a code generator.

use crate::runtime::{Instance, Store, Trap};
use std::cell::Cell;

/// State shared by native code, its helpers and the interpreter.
#[repr(C)]
pub struct JitCtx {
    pub regs: *mut u64,
    pub mem: *mut u8,
    pub mem_len: u64,
    /// Pointers to the value cells of the instance's globals.
    pub globals: *const *mut u64,
    /// Back-edges until the next interrupt poll.
    pub fuel: u64,
    /// Native entries of the instance's functions (null: not compiled).
    pub entries: *const Cell<*const u8>,
    pub store: *const Store,
    pub inst: *const Instance,
    /// `*mut &mut dyn Host`
    pub host: *mut u8,
    /// Error of a failed helper (boxed `Trap`).
    pub trap: *mut Trap,
    pub depth: *mut usize,
    pub max_depth: u64,
    /// End of the value stack.
    pub stack_end: *mut u64,
    /// Lowest native stack pointer allowed.
    pub native_limit: u64,
    pub fuel_reset: u64,
}

#[cfg(target_arch = "x86_64")]
#[path = "jit_x64.rs"]
mod native;
#[cfg(target_arch = "x86_64")]
pub use native::{call, compile, compiled, enabled, Compiled};

#[cfg(not(target_arch = "x86_64"))]
mod fallback {
    use super::JitCtx;
    use crate::ir::Code;
    use crate::runtime::Trap;

    /// No native code can be constructed on this architecture.
    #[derive(Debug)]
    pub enum Compiled {}

    impl Compiled {
        pub fn entry(&self) -> *const u8 {
            match *self {}
        }
    }

    pub fn enabled() -> bool {
        false
    }

    pub fn compiled(_code: &Code) -> Option<&Compiled> {
        None
    }

    pub fn compile(_code: &Code) -> Option<Compiled> {
        None
    }

    /// Native execution is unavailable; callers must use the interpreter.
    ///
    /// # Safety
    /// This fallback never dereferences the context or native pointers.
    pub unsafe fn call(_ctx: &mut JitCtx, _regs: *mut u64, _entry: *const u8) -> Result<(), Trap> {
        Err(Trap::Other("native code is unavailable on this architecture".into()))
    }
}
#[cfg(not(target_arch = "x86_64"))]
pub use fallback::{call, compile, compiled, enabled, Compiled};

/// Native entry cells of an instance's functions.
pub fn entry_cells(n: usize) -> Box<[Cell<*const u8>]> {
    (0..n).map(|_| Cell::new(std::ptr::null())).collect()
}

