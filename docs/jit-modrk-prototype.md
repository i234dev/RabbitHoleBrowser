# Isolated ModRK prototype: pending root validation

Base: published `90e464f3c603617b718b9bd41568e48d79ddccd5` (0.1.16).
No Cargo/Rust, SSH, native execution or performance measurement was performed
while preparing this candidate. The written tests below are **not passes**.
The rejected cached-simple `2e468` and its ten semantic checks, 864 samples,
14 case/mode regression flags and inconclusive throughput case stay intact.
This branch does not contain or rescue that shortcut.

## Current phases and remaining gates

Stage 2 is the implementation stage, not "phase 2". The current baseline
has the following functionality; its activation/performance gates remain
separate from implementation coverage.

| Phase | Present on the base | Remaining limitation/gate |
|---|---|---|
| 1: infrastructure | Own x86-64 assembler, W^X executable memory, source-bound differential tools and code budgets | Preserve semantic parity and measure the off control for every change |
| 2: numeric templates | Register/constant operands, arithmetic and comparisons | Original compilation-cost objective remains open; this prototype only changes a subset of one existing opcode |
| 3: control flow | Branches, fused latches, backward-edge ticks and watchdog | Preserve interruption and GC-stress behaviour |
| 4: dynamic values | Native IC/scope/reference-count paths and runtime fallbacks | The historical million-program result belongs to `8fd6578`; it is not exhaustive conformance or validation of this branch |
| 5: calls/exceptions | JS/native calls, constructors, return/error handling and native frame switches | Cached-simple was rejected. Windows has explicit native canaries, but normal selection remains disabled; the pure unwind descriptor is not production OS table registration |
| 6: tiering/memory | Threshold1000, entry minimum24, failure caching, per-thread16MiB code cap and release with FunctionCode | Compilation/page costs and page-load goals remain open; the measured arena experiment was rejected |
| 7: loop OSR | Native continuation from a live interpreted loop frame | Preserve mid-function entry, slots, safepoints and exceptions |
| 8: performance | Native property/global/array/scope paths and lighter helpers | Repeated global improvement and absence of losses are not established by this branch or by a single numeric loop |
| GC extension | Incremental barriers shared with the interpreter | Preserve live references, barrier behaviour and real pause measurements |

The JIT stays off by default. `emit::PHASE=5` selects instruction families;
it is not a progress percentage. There is no JavaScript AArch64 emitter on
the base. ARM native emission and an optimizing/register-allocation tier are
separate projects, not implicit parts of this change. Historical statements
and old pending sections in jit-design.md do not substitute for current
source-bound validation receipts.

## Actual cost and admitted subset

The current numeric ModRK path always calls `h_mod`, which invokes `js_mod`.
That runtime helper reconverts/checks both operands, including the divisor
already known to the compiler. The existing corpus contains `%7`
(`global_loop`), `%1000003` (`hot_numeric`) and `%3`/`%5` (`hot_branchy`).
The design's older ModRK profile is historical; no fresh opcode attribution
or percentage speed claim is made here.

Only a constant Number divisor that is an exact integer in `[1, i32::MAX]`
selects the new template. It admits only exact Number dividends in
`[+0, i32::MAX]`. Its sign-bit guard rejects every negative, including `-0`.
The range/roundtrip guards reject fractions, NaN, infinities and larger
values. They branch to the original pure `h_mod` with original XMM0/XMM1.
Non-Number dividends retain `h_exec`/ToNumeric/coercion/errors unchanged.
All other divisors and ModRR keep their previous paths.

The divisor is still loaded from the FunctionCode constant pool. No
JavaScript numeric constant is embedded as an instruction immediate.
The admitted arithmetic avoids the helper call/return and redundant dynamic
divisor checks; actual timing benefit is unknown. Additional guards and
code size can outweigh it, and source invariance does not guarantee an
unchanged off-mode executable layout or performance.

## Instruction, alias and ABI contract

`DIV r/m32` uses EDX:EAX, returns quotient in EAX and remainder in EDX, and
leaves arithmetic flags undefined. The encoder uses F7 /6 without REX.W,
with REX.B only for high divisor registers. The template sets EDX=0 and
proves `0 <= EAX <= i32::MAX` and `1 <= ECX <= i32::MAX`; the quotient fits
in32bits and the divisor is nonzero, excluding both divide-error causes.
The nonnegative remainder fits signed31bits and converts exactly to f64.

Scratch state is RAX/RCX/RDX and XMM0..2. The existing fixed/saved GPRs,
RSP/prologue, frame PC, slot base and safepoint schedule stay unchanged.
There is no call on the admitted arithmetic path. The fallback continues
to use the existing explicit SysV64 helper ABI, also on Windows, with
XMM0/XMM1 arguments and XMM0 return; all changed scratch registers are
volatile under that ABI and also under Win64. No Windows selector/unwind
change is included.

Both operands are loaded before any destination write. All guards preserve
source slots and both original floating operands. The shared store/release
machinery handles `d==a`, distinct destinations and old counted values.
Test-only direct bytecode fixes those operand aliases explicitly rather
than assuming what the frontend's register allocator happens to emit.

Native witnesses are compiled only for tests: the admitted branch increments
a thread-local Cell after its shared store; h_mod records actual pure-helper
calls. The branch witness uses a scratch register and a memory add, with no
helper/allocation/safepoint. FunctionCode/Realm are Rc-owned on their thread.
No witness or field/layout change exists in release builds.

Primary contracts: [ECMAScript Number::remainder](https://tc39.es/ecma262/multipage/ecmascript-data-types-and-values.html#sec-numeric-types-number-remainder),
[Intel SDM Volume2A, DIV](https://cdrdv2.intel.com/v1/dl/getContent/671199)
(order253666-093US, section DIV, printed3-264 onward), and the existing
explicit helper ABI in jit/helpers.rs. In particular, zero keeps the
dividend's sign in the general remainder operation; rejecting negative
inputs ensures the new unsigned subset cannot erase `-0`.

## Written controls and root commands

Two encoder controls cover all16 divisor registers/listing and an executable
SysV64 leaf stub. The latter passes deliberately nonzero high halves, checks
32-bit quotient/remainder zero-extension, divisor/input preservation and
use of volatile registers only. It does not test production Windows unwind.

Seven JS controls cover exact boundaries/signed zero, other divisors,
conversion order/throws/BigInt/Symbol, native fast and numeric fallback
witnesses at thresholds0/2, explicit slot aliases/shared strings and
BigInts, excluded-divisor native fallback, and actual GC during object
conversion. VM controls and native results are compared exactly, including
negative zero. The native alias test returns source+result for `d!=a`, so a
clobbered source cannot silently pass. These controls are all unexecuted.

Root's smallest commands, after snapshot verification and its resource guard:

```text
cargo test -p rabbithole-jit unsigned_divide --locked
cargo test -p rabbithole-js --features jit modrk_ --locked
```

Root can repeat the JS selection under the existing full/inc GC stress
settings and then the established JS/JIT suites and differential harness.
No new timing framework, corpus, gate, default flag or environment switch
is introduced. Root will first validate this exact source and mechanism,
then choose a predeclared existing perf subset against the real0.1.16
baseline, including unrelated off/on controls. A fresh full864-row attempt
is not an automatic next step, and no improvement or integration is claimed.
