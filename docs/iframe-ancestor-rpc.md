# Inactive iframe ancestor calls: isolated candidate

This candidate extends the existing Engine cross-realm bridge for global
property reads and function calls from an iframe to a reachable, inactive
ancestor or sibling. It is based on source 34780e675d84a769bfb427e07af34d9ba053a0ae.
The source changes and twenty-seven tests (seventeen Engine, five JS, five Script) have not been
compiled or executed yet.

The reverse route accepts only HTTP(S) documents whose effective origin equals
their URL's non-opaque tuple origin, and whose tuple origins agree. This is a
conservative boundary, not a replacement for per-document origin identity.
It rejects inherited about:blank/srcdoc documents, data documents and other
schemes. Existing descendant permissions and frame limits are not broadened.

A source realm lends an opaque watchdog state to the target: the existing
absolute deadline, its original CPU clock domain, and termination state.
GetGlobal includes property getters. Descendant Document/Dispatch/Method calls
use the same loan so nested host calls cannot restart the task. The target
temporarily disables task-budget restart. RAII returns its effective state
to the caller, including a deadline shifted by consumed host credit and any
termination, then restores only the inactive target's original state/budget.
An expired idle target deadline never replaces the source's live task.

Thread-local host credit and regexp deadline/expiration remain shared by the
running task. Loan entry/exit never resets, duplicates or restores those TLS
values: credit consumed by the target is represented in the returned deadline;
unconsumed/new credit remains pending for the source. CPU used in the target
continues advancing the same task clock. The new Realm helpers copy scalar
state; they do not change VM/JIT loops, ABI or watchdog polling rules. Custom
hosts implementing only the old HostEnv method retain their legacy behavior
through the default forwarding method.

The module follow-up removes the old dependency-compilation deadline restart.
Production EngineEnv::fetch in crates/engine/src/scripting.rs already credits
its measured synchronous request duration with exclude_from_watchdog. The JS
module branch starts its separate timer immediately before compile_module and
credits that interval on success or syntax error when a deadline is enabled.
It does not credit JSON/CSS processing: JSON.parse can execute page code.
Disabled deadlines remain disabled. No network timing or module fetch policy
is changed.

Three module tests use an explicit MockEnv host-credit fixture without I/O or
sleep. They verify unchanged task state, pending credit consumed exactly once,
compilation credit bounded by the measured loader interval, syntax failure,
and a real borrowed global call entering the dependency loader synchronously
through a test native. Both successful and failed compilation restore the
idle target and return the source state. These are deterministic local binding
fixtures, not real-network performance measurements.

Executing frame pages are detached from their owners, including sibling
calls. Restoration and subsequent input/tick/message effects resolve stable
context IDs rather than the original vector index. Detached contexts cannot
be replaced by an accidental initial blank document. Removing a context
discards its detached result instead of replacing the next sibling. New
frame document installation follows the same ownership boundary.

A checkpoint requested inside a foreign call remains pending. Reverse-call
jobs wait until the owner and suspended source hosts are restored. Descendant
jobs retain their checkpoint between consecutive classic scripts. A nested
ancestor call can touch a sibling outside the source child's tree; that
sibling's checkpoint remains pending until its owner is reachable. Queues
belong to each Engine's FrameRegistry, and a per-Engine wakeup flag shared by
its Pages covers input callbacks returning without settle. Another Engine
cannot steal the work or consume that flag. Bounded checkpoints retain the
ID/wakeup while Promise, mutation or custom-element/slot work remains.

Each queued checkpoint additionally carries a Weak document/realm token.
Exactly one non-Clone Page owns the strong token: moves and temporary
frame detachment preserve it; every Page construction gets a fresh token,
including replacement with the same FrameId. DOM/render snapshots do not
contain it. document.open retains the existing realm and token. Queues cannot
keep a discarded Page alive. Dead jobs are pruned before the no-ScriptHost
gate, and a live but superseded token never runs against the new document's
host. Preparing and cancelling a provisional Page leaves the current Page's
jobs intact. These ownership controls exercise Engine Pages directly; they
do not claim a new GUI navigation/cancellation integration test.

Separate realm queues remain: this candidate does not implement a single
global FIFO microtask queue across all realms, and does not claim complete
event-loop conformance. An ancestor whose ScriptHost is already active stays
unavailable. Calling back into that active ancestor is not enabled by aliasing
or re-entering its borrowed realm. Values still cross realms through the
existing structured clone bridge, with its function/DOM/object limitations.
This is not a complete WindowProxy implementation or a demonstrated
authentication fix.

Engine tests use real Engine and ScriptHost instances and in-memory HTML at
fixture HTTP URLs. They cover inactive globals/calls, Promise boundaries,
classic-script ordering, nested sibling jobs, active-host refusal,
cross-origin getter exclusion, sibling detachment, identity after
reorder/removal, JS removal of the source frame, borrowed deadline recovery,
getter failure, focus/key wakeup, multiple Engines and partial checkpoints.
JS tests cover original CPU clock identity, consumed/pending credit,
termination propagation, target-state restoration and regexp TLS preservation.
Script tests check caller adoption during a Rust host unwind with no JS or
generated frame active, followed by a new task that recovers; an explicitly
disabled budget/deadline remains disabled. ScriptHost's global panic catch
retains the effective watchdog around stack restoration while keeping the
original panic report and VM/root unwinding. This provides no native unwind
evidence. The earlier d5578cc checkpoint's new assertion was statically blocked:
its catch restored termination from the old stack mark; this follow-up fixes
that behavior rather than treating that earlier source as passing.
No account/site authentication was tested. Opaque origins, named-global
lookup and deeper/wider frame limits remain separate work.
