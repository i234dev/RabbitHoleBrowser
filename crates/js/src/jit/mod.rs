//! Baseline compiler ("Level 1", docs/jit-design.md).
//!
//! Hot functions are translated whole into x86-64 templates over the
//! interpreter's own frame: every value lives in its `vstack` slot, so
//! native code and the interpreter can hand a frame to each other at any
//! instruction boundary without reconstructing state. Native code never
//! calls JavaScript recursively on the native stack: calls and returns
//! leave to the run loop, which continues with the callee (interpreted or
//! compiled) and later re-enters the caller's code at its resume point.
//!
//! Runtime switches (environment; the browser maps `--jit=on|off` and
//! `--jit-threshold=N` to them):
//!
//! * `RABBITHOLE_JIT=on|off` (default off)
//! * `RABBITHOLE_JIT_THRESHOLD=N`: hotness needed to compile (calls, plus
//!   loop iterations weighted through the safe-point tick); 0 compiles
//!   every function the first time it runs
//! * `RABBITHOLE_JIT_TRACE=1`: report compiled and rejected functions
//! * `RABBITHOLE_JIT_DUMP=name`: listing of the native code of functions
//!   whose name contains `name` (`*`: all)
//! * `RABBITHOLE_JIT_EXCLUDE=a,b` / `RABBITHOLE_JIT_ONLY=a,b`: function
//!   name patterns (substring; `name@file` also matches the file)
//! * `RABBITHOLE_JIT_MAX=n`: compile at most n functions (bisection)
//! * `RABBITHOLE_JIT_STATS=1`: coverage report when the realm is dropped
//!   (share of safe-point samples in hot / compiled functions, functions
//!   compiled, code bytes)

use crate::bytecode::FunctionCode;
use crate::realm::Realm;
use std::cell::{Cell, OnceCell};

#[cfg(target_arch = "x86_64")]
mod emit;
#[cfg(all(test, target_arch = "x86_64"))]
pub(crate) use emit::PHASE;
#[cfg(all(test, not(target_arch = "x86_64")))]
pub(crate) const PHASE: u8 = 0;
mod debug;
mod helpers;


/// Process-wide options, read once from the environment.
#[derive(Debug, Clone)]
pub struct Options {
    pub on: bool,
    pub threshold: u32,
    pub trace: bool,
    pub dump: Option<String>,
    pub exclude: Vec<String>,
    pub only: Vec<String>,
    pub max: Option<u32>,
    pub stats: bool,
    pub gc_stress: bool,
}

/// Default hotness threshold (calls; a loop adds [`LOOP_WEIGHT`] per
/// safe-point tick it is caught in).
pub const DEFAULT_THRESHOLD: u32 = 1000;
/// Hotness added to the running function at each safe-point tick (every
/// 1024 backward jumps or calls in the realm).
pub const LOOP_WEIGHT: u32 = 100;
/// Loop OSR: a backward jump caught by the interpreter's safe-point tick
/// in a hot function continues the frame in native code (stage 2 phase 7).
pub(crate) const LOOP_OSR: bool = false;
/// Functions longer than this stay interpreted.
pub const MAX_OPS: usize = 20_000;
/// Frames larger than this stay interpreted (slot offsets fit in disp32
/// comfortably, and untranslated code is excluded).
pub const MAX_FRAME: usize = 60_000;
/// Native code per thread beyond which nothing more is compiled.
pub const MAX_CODE_BYTES: usize = 16 << 20;

pub fn options() -> &'static Options {
    static O: std::sync::OnceLock<Options> = std::sync::OnceLock::new();
    O.get_or_init(|| {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let list = |k: &str| var(k).map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()).unwrap_or_default();
        let on = matches!(var("RABBITHOLE_JIT").as_deref(), Some("on" | "1" | "true" | "yes"));
        Options {
            // Native code is only generated where it has been tested.
            on: on && rabbithole_jit::supported() && cfg!(not(windows)),
            threshold: var("RABBITHOLE_JIT_THRESHOLD").and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_THRESHOLD),
            trace: var("RABBITHOLE_JIT_TRACE").is_some(),
            dump: var("RABBITHOLE_JIT_DUMP"),
            exclude: list("RABBITHOLE_JIT_EXCLUDE"),
            only: list("RABBITHOLE_JIT_ONLY"),
            max: var("RABBITHOLE_JIT_MAX").and_then(|v| v.parse().ok()),
            stats: var("RABBITHOLE_JIT_STATS").is_some(),
            gc_stress: var("RH_GC_STRESS").is_some(),
        }
    })
}

/// Compilation state of a function.
pub(crate) const UNTRIED: u8 = 0;
pub(crate) const COMPILED: u8 = 1;
pub(crate) const REJECTED: u8 = 2;

/// Per-function state ([`crate::bytecode::JitSlot`]).
#[derive(Default)]
pub struct Slot {
    /// Entry point of the native code (0: none).
    pub(crate) entry: Cell<usize>,
    /// Hotness.
    pub(crate) counter: Cell<u32>,
    pub(crate) state: Cell<u8>,
    /// Owner of the native code: freed with the `FunctionCode`, which
    /// outlives every frame running it.
    pub(crate) code: OnceCell<JitCode>,
}

/// Native code of one function.
pub struct JitCode {
    #[allow(dead_code)]
    mem: rabbithole_jit::ExecMem,
}

/// Per-realm state.
pub(crate) struct State {
    /// Compile and run native code.
    pub on: bool,
    /// Count hotness (JIT on, or statistics wanted).
    pub active: bool,
    pub threshold: u32,
    /// A panic caught in a helper, resumed once native code has returned.
    pub panic: Option<Box<dyn std::any::Any + Send>>,
    pub stats: Stats,
    /// `RH_GC_STRESS`: collect at every safe point, one tick apart.
    pub gc_stress: bool,
    /// Entries into native code.
    pub entries: u64,
}

#[derive(Default)]
pub(crate) struct Stats {
    pub enabled: bool,
    /// Safe-point samples: all, in functions past the threshold, in
    /// native code.
    pub samples: u64,
    pub hot_samples: u64,
    pub jit_samples: u64,
    pub compiled: u32,
    pub rejected: u32,
    pub code_bytes: usize,
    pub ops_compiled: usize,
    pub compile_ns: u128,
    /// Entries into native code.
    pub entries: u64,
}

impl Default for State {
    fn default() -> State {
        let o = options();
        debug::install_fault_reporter();
        State { on: o.on, active: o.on || o.stats, threshold: o.threshold, panic: None, stats: Stats { enabled: o.stats, ..Default::default() }, gc_stress: o.gc_stress, entries: 0 }
    }
}

impl Drop for Stats {
    fn drop(&mut self) {
        if !self.enabled || self.samples + self.entries + self.compiled as u64 + self.rejected as u64 == 0 {
            return;
        }
        let pct = |n: u64| if self.samples == 0 { 0.0 } else { 100.0 * n as f64 / self.samples as f64 };
        eprintln!(
            "[jit] samples {} | in hot functions {:.1}% | in native code {:.1}% | compiled {} ({} ops, {} bytes, {:.2} ms, {:.3} us/op; mapping {:.2} ms in this thread) | rejected {} | entries {}",
            self.samples,
            pct(self.hot_samples),
            pct(self.jit_samples),
            self.compiled,
            self.ops_compiled,
            self.code_bytes,
            self.compile_ns as f64 / 1e6,
            if self.ops_compiled == 0 { 0.0 } else { self.compile_ns as f64 / 1e3 / self.ops_compiled as f64 },
            MAP_NS.with(|m| m.get()) as f64 / 1e6,
            self.rejected,
            self.entries
        );
    }
}

thread_local! {
    /// Native code bytes of this thread (realms are per thread).
    static CODE_BYTES: Cell<usize> = const { Cell::new(0) };
    /// Time spent mapping and protecting native code (part of compile time).
    pub(crate) static MAP_NS: Cell<u64> = const { Cell::new(0) };
    /// Functions compiled by this thread (for `RABBITHOLE_JIT_MAX`).
    static COMPILED_COUNT: Cell<u32> = const { Cell::new(0) };
}

impl Realm {
    /// Override the process-wide options for this realm: compile and run
    /// native code (`on`), with hotness `threshold` (0: everything at
    /// first run). For differential tests and tools.
    pub fn set_jit(&mut self, on: bool, threshold: u32) {
        self.jit.on = on && rabbithole_jit::supported();
        self.jit.active = self.jit.on || self.jit.stats.enabled;
        self.jit.threshold = threshold;
    }

    /// Is native code enabled for this realm?
    pub fn jit_enabled(&self) -> bool {
        self.jit.on
    }

    /// Entries into native code so far (calls, resumes, loop OSR).
    pub fn jit_entries(&self) -> u64 {
        self.jit.entries
    }
}

/// Native code bytes currently mapped by this thread.
pub fn code_bytes() -> usize {
    CODE_BYTES.with(|c| c.get())
}

impl Drop for JitCode {
    fn drop(&mut self) {
        let n = self.mem.mapped_len();
        CODE_BYTES.with(|c| c.set(c.get().saturating_sub(n)));
    }
}

fn matches(pats: &[String], code: &FunctionCode) -> bool {
    let name = code.name.to_rust_string();
    pats.iter().any(|p| match p.split_once('@') {
        Some((n, f)) => name.contains(n) && code.filename.contains(f),
        None => name.contains(p.as_str()) || (p == "*"),
    })
}

/// Why a function is not compiled (None: it can be).
fn exclusion(code: &FunctionCode) -> Option<String> {
    let o = options();
    if code.lazy.is_some() {
        return Some("lazy".into());
    }
    if code.flags.is_generator || code.flags.is_async {
        return Some("generator/async".into());
    }
    if code.ops.len() > MAX_OPS {
        return Some(format!("{} ops", code.ops.len()));
    }
    if code.frame_size() > MAX_FRAME {
        return Some(format!("frame of {} slots", code.frame_size()));
    }
    if !o.only.is_empty() && !matches(&o.only, code) {
        return Some("not in RABBITHOLE_JIT_ONLY".into());
    }
    if matches(&o.exclude, code) {
        return Some("RABBITHOLE_JIT_EXCLUDE".into());
    }
    if let Some(max) = o.max {
        if COMPILED_COUNT.with(|c| c.get()) >= max {
            return Some("RABBITHOLE_JIT_MAX reached".into());
        }
    }
    if code_bytes() >= MAX_CODE_BYTES {
        return Some("code memory limit".into());
    }
    None
}

fn describe(code: &FunctionCode) -> String {
    let name = code.name.to_rust_string();
    let line = code.positions.first().map(|p| p.line).unwrap_or(0);
    format!("{}@{}:{}", if name.is_empty() { "<anonymous>" } else { &name }, code.filename, line)
}

/// Try to compile `code` (once): sets its entry on success.
#[inline(never)]
pub(crate) fn compile(code: &FunctionCode, stats: &mut Stats) {
    let slot = &code.jit.s;
    if slot.state.get() != UNTRIED {
        return;
    }
    let o = options();
    let t0 = std::time::Instant::now();
    let r = match exclusion(code) {
        Some(why) => Err(why),
        None => build(code),
    };
    let dt = t0.elapsed();
    match r {
        Ok((mem, listing)) => {
            let bytes = mem.mapped_len();
            CODE_BYTES.with(|c| c.set(c.get() + bytes));
            COMPILED_COUNT.with(|c| c.set(c.get() + 1));
            stats.compiled += 1;
            stats.code_bytes += mem.len();
            stats.ops_compiled += code.ops.len();
            stats.compile_ns += dt.as_nanos();
            if o.trace {
                eprintln!("[jit] compiled {} ({} ops, {} bytes, {} us)", describe(code), code.ops.len(), mem.len(), dt.as_micros());
            }
            if let Some(l) = listing {
                eprintln!("[jit] ---- {} ({} ops, {} bytes at {:p})\n{l}", describe(code), code.ops.len(), mem.len(), mem.ptr());
            }
            slot.entry.set(mem.ptr() as usize);
            let _ = slot.code.set(JitCode { mem });
            slot.state.set(COMPILED);
        }
        Err(why) => {
            stats.rejected += 1;
            if o.trace {
                eprintln!("[jit] rejected {}: {why}", describe(code));
            }
            slot.state.set(REJECTED);
        }
    }
}

/// Generate the native code of `code` (and its listing when dumping).
fn build(code: &FunctionCode) -> Result<(rabbithole_jit::ExecMem, Option<String>), String> {
    #[cfg(target_arch = "x86_64")]
    {
        let o = options();
        let dump = o.dump.as_ref().map(|d| d == "*" || code.name.to_rust_string().contains(d.as_str())).unwrap_or(false);
        emit::compile(code, dump)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = code;
        Err("no code generator for this architecture".into())
    }
}
