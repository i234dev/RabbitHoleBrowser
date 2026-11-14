# Nested Worker reply ports

This candidate repairs transfer lists on messages sent through an already
transferred Worker MessagePort. Both owner and worker forwarding previously
serialized only data, and both port-message receivers omitted reconstruction
of nested ports. Dedicated Worker bootstrap transfer and same-realm MessagePort
transfer already worked.

The test-only baseline 9cc7070 was executed by the validation owner with
interpreter mode, incremental GC stress and a single test thread. The local
control passed. Owner→worker and worker→owner controls failed with port counts
zero, respectively reporting worker-stage 0 true undefined undefined and
owner-stage 0 true. This was a targeted diagnostic, not release/workspace
validation.

The forwarding envelopes now include nested port bridge identifiers. Existing
initial-transfer helpers are reused for sending and receiving those lists,
retaining their order and using real local endpoints for each transported
capability. No dummy port substitutes a missing endpoint. Each regression
requires both the transferred endpoint and an actual reply through it.

Worker addressing is unchanged: owner bridge IDs use a realm-local monotonic
sequence; worker bridge IDs include the native worker instance ID and its
local sequence. Normal worker creation allocates a new ID, and terminating a
dedicated worker removes its runtime. Bridge-map entries remain retained until
the owner realm dies; terminate does not purge them. Monotonic IDs describe
ordinary tested cases, not exhaustion of the Rust u32 or JS Number counters.
Added controls exercise simultaneous
worker instances, SharedWorker connections whose native delivery uses a
canonical alias, and stale endpoints after terminate/recreate. No new gate
assumes that every SharedWorker connection object has the canonical ID.

Seven controls are defined: the original local positive, both failing baseline
directions, independent instances, SharedWorker replies, terminate/recreate,
and an endpoint relayed from Worker A through the owner to Worker B with a
real B→A→B roundtrip.
They have not yet been compiled or executed against this fix at commit time.
The Gmail iframe fix remains separate and unchanged.

This change does not claim complete transferable detachment/ownership,
transfer-list validation, ports inside cloned payload data, origin/authentication
hardening, bridge-map reclamation, or counter-exhaustion handling. SharedWorker
sharing retains its existing owner-realm scope. It does not integrate ServiceWorker client messaging, WindowProxy or iframe
navigation behavior.
