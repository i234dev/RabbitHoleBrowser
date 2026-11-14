# Nested transferred MessagePorts in iframe messages

An iframe MessagePort bridge must carry the transfer list of each message,
including messages sent over an already transferred port. The initial window
message already transported its ports, but the port-message forwarding path
serialized only data and discarded event.ports. A request/reply protocol that
attaches a fresh reply port to each request therefore received no reply port.
Calling postMessage on that missing entry produced an undefined-receiver error.

The public static Google widget bundle inspected on 2026-11-14 exhibits this
protocol: its message callback constructs a reply adapter from ports[0], then
its send method posts through that adapter. The reported stack points to a
MessagePort callback dispatched by messaging.js, not a missing WindowProxy.
This identifies a concrete matching failure, not authenticated Gmail success.

A local HTTP fixture reproduced the exact undefined.postMessage error using
published 0.1.14 source fc4c3730c64fecdb25acd9c30ffb9c37432a1f74. The receiver
observed zero transferred reply ports while the source WindowProxy remained
the expected iframe and its postMessage member remained a function. Port
events correctly had source=null. The reproduction used interpreter mode and
incremental GC stress, one browser, CPU 2/nice 19, 800000 KiB address-space limit,
20 s wall deadline and the shared local conformance lock. No account or external
site was opened. Private evidence retains original fixture bytes, logs,
checksums and the official binary identity.

The forwarding envelope now includes bridges for nested event.ports. Both
initial window messages and port messages reconstruct those endpoints in the
receiving realm, retaining port order and leaving empty transfer lists empty.
No JavaScript values cross realms directly; the existing serialized frame
transport carries data and bridge identifiers. MessageEvent.source is not
substituted for a missing port, and no dummy ports are created.

Two Engine tests are defined for three request/reply transfer hops in both
directions, independent transferred endpoints, empty transfer lists, and
separation of MessagePort.source from the iframe WindowProxy source. The same
standalone fixtures can be served over loopback. They have not yet been
compiled or executed against this candidate at commit creation time.

Worker bridges have the analogous nested-transfer limitation and are outside
this change. General transferable ownership/detachment, transfer-list
validation, ports embedded inside cloned data, and bridge-map reclamation are
not broadened or claimed conformant here.

A separate commit namespaces bridge identifiers per ScriptHost frame-factory
installation using an exact string from a checked, process-wide atomic counter.
FrameId and origins remain unchanged. The allocator fails closed instead of
wrapping; no floating-point ID conversion loses precision. A navigation test
retains a live old endpoint, replaces the iframe document while preserving its
FrameId, and verifies that only the fresh endpoint reaches the new realm.
WindowProxy source identity remains stable, while MessagePort source remains
null. A pure allocator test covers exact values beyond 2^53 and exhaustion with
an isolated counter. These additional candidate tests are not yet executed.
