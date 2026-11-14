# Inactive iframe ancestor calls: isolated candidate

This candidate extends the existing Engine cross-realm bridge for global
property reads and function calls from an iframe to a reachable, inactive
ancestor or sibling. It is based on source 34780e675d84a769bfb427e07af34d9ba053a0ae.
The source changes and ten tests have not been compiled or executed yet.

The reverse route accepts only HTTP(S) documents whose effective origin equals
their URL's non-opaque tuple origin, and whose tuple origins agree. This is a
conservative boundary, not a replacement for per-document origin identity.
It rejects inherited about:blank/srcdoc documents, data documents and other
schemes. Existing descendant permissions and frame limits are not broadened.

A source realm passes its existing absolute deadline through GetGlobal and
CallGlobal, including property getters. The target temporarily disables task
budget restart, uses the caller's unchanged deadline (an idle target's previous deadline may
already have expired), then
restores its idle deadline, budget and the caller's regexp deadline by RAII.
No VM or JIT behavior is changed. Hosts implementing only the old HostEnv API
retain their existing behavior through the default forwarding method.

Executing frame pages are detached from their owners, including sibling
calls. Restoration and subsequent input/tick/message effects resolve stable
context IDs rather than the original vector index. Detached contexts cannot
be replaced by an accidental initial blank document. Removing a context
discards its detached result instead of replacing the next sibling. New
frame document installation follows the same ownership boundary.

A checkpoint requested inside a foreign call remains pending. Engine drains
reverse-call jobs only after the owner and suspended source hosts are restored;
settle/tick provides that boundary. Descendant jobs retain their existing
checkpoint between consecutive classic scripts through a separate queue. This preserves completion of the source
synchronous stack before ancestor jobs. Separate realm queues remain: this
candidate does not implement a single global FIFO microtask queue across all
realms, and does not claim complete event-loop conformance.

An ancestor whose ScriptHost is already active remains unavailable. Calling
back into that active ancestor is not enabled by aliasing or re-entering its
borrowed realm. Values still cross realms through the existing structured
clone bridge, with its existing function/DOM/object limitations. This is not
a complete WindowProxy implementation or a demonstrated authentication fix.

The tests use real Engine and ScriptHost instances and in-memory HTML at
fixture HTTP URLs. They cover inactive global reads/calls, deferred Promise
callbacks, refusal of an active ancestor, cross-origin getter exclusion,
sibling detachment, identity after reorder/removal, a JS callback removing
its source iframe, and bounded getter/function execution followed by recovery.
They do not constitute browser-site or account testing. Opaque origins,
named-global lookup, deeper/wider frame limits, and broader proxy security
remain separate work.
