# Isolated cached-simple call prototype

Status: source written; independent peer review and all runtime gates pending.
No candidate Rust build,
execution or timing result is claimed. Base `bf65b98a335dd1a0eeaf0d75d887ebcc67e96bf0`.
The regular JIT remains off by default, including the Windows environment gate.
This prototype does not close phase 8 or add an ARM backend.

The preserved D81 profiles and later 673/76/89 native-PC diagnostics identify
`h_call`/`h_return` as frequent work in recursive/call microcases. They do not
identify individual source operations, and the later PC capture reports roughly
46% timer overruns. The historical `h_call` symbol was 13,725 bytes, including
inlined full `CallR` dispatch; that is not a measurement of this base or candidate.
M (`db11bb1`) and L (`90923a8`) remain rejected by their complete frozen gates.
The two/five adverse cases cannot be cancelled by their positive call timings.
This change does not reintroduce their metadata, slot compaction or classification
changes, nor the older direct-resume/arena experiments.

The hypothesis is instruction working-set cost on the warm monomorphic path.
The executed `h_call` now admits only a cache hit satisfying all of:

- Identical callee bits and unchanged collection epoch in the existing `CallIc`.
- A recorded scope and a live code owner under that existing cache contract.
- Callee realm is current, `CALL_SIMPLE` is set, and the callee is not an arrow.
- Receiver binding is either strict, nullish/global, or an existing object;
  sloppy primitive boxing stays on the full path.

It synchronizes the caller PC before a possibly failing push and invokes the
same `push_simple_frame` as the VM. Thus argument moves/clones, undefined slots,
stack/frame growth, maximum depth, this/new.target, roots and return slots remain
shared. The complete existing `Realm::call_r` is behind an out-of-line wrapper
for every miss, including lazy/scope-allocating/arrow/bound/proxy/native calls.
Cold misses perform a preliminary cache probe and may cost more. Whether the
smaller hot helper compensates for that is explicitly unmeasured.

The emitter and its call tick, the panic-catching helper guard, `status`,
`switch_frame`, `ReturnR`, native entry/prologue/epilogue and unwind recipes are
unchanged. Successful pushes still use the shared frame switch to reload slots,
constants, code and PC, without growing the native stack per JS call. Exceptions
still leave native code and use the VM's unwind path. No new persistent pointer,
GC edge, code owner, call IC or frame field is introduced. Test-only counters
record successful pushes and errors; they are absent from production layouts.

Five new test groups cover exact per-invocation hit/miss evidence across GC and
closure identity changes, eight independent differential fixtures in off/0/3
and ordinary/full/incremental GC modes, actual warm-push stack growth, a failed
maximum-depth push followed by catch/finally/success, and a watchdog interruption
that must execute this new path. Existing native-callback/getter GC, panic,
counted/wide return, cross-realm and Windows override canaries remain required.
Windows pure unwind encoding is not OS unwind-registration or stack-walk proof;
the machine-stack recipe is not changed by this prototype.

Before accepting a performance claim, build both exact revisions using matching
compiler/profile/flags and archive source/ELF hashes. Inspect the final helper
and fallback bodies to verify that the intended split survived compilation.
Run correctness/differential/GC/watchdog and platform gates first, then compare
all 27 perf cases in off/on modes in a complete balanced quiet block. Retain
cold-call, native-heavy, bound/proxy, strings/BigInt and no-call negative controls,
RSS, code/mapping bytes and compile costs. An isolated call win does not justify
a broad regression. Browser/Speedometer measurements need a fresh comparable
final-artifact protocol; no old binary result transfers to this revision.
