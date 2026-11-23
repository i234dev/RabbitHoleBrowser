# Browser API diagnostic controls, 23 November 2026

RabbitHole ran these controls on TheOtherSide against WPT `831baf28d226911e47b2454d69e0c9e7348dd4d1`. The reference was `a43c8e0`. The form measurement used `6876b713`; the messaging, observer and WebAssembly controls used `e6850ee8`. The latter retained the form changes. All network traffic used private loopback. The complete browser inventory contains 40,172 URLs.

## Form submission and character encoding

A legacy-encoded form targeted a named child frame. The previous engine could encode the data but sent the navigation to the parent. The child's WindowProxy also lacked a readable `location.search`. The harness waited for navigation before checking each character. Routing the request to the named child and exposing its live same-origin URL let the tests read the encoded bytes.

The EUC-KR page covering characters 1 through 1,000 went from 1,000 subtests left pending to 1,000 passes. The following independent run covered all 1,311 pages discovered by the native runner under `encoding`. Both revisions used ten workers, a five-second timeout, 15 seconds for long cases and the same local HTTP fixtures. The candidate completed in 657 seconds.

| Native directory | Pages | Reference passed / reported | Candidate passed / reported |
|---|---:|---:|---:|
| encoding, direct | 81 | 18,772 / 23,416 | 22,892 / 23,249 |
| encoding/legacy-mb-japanese | 482 | 129,771 / 333,666 | 333,460 / 333,666 |
| encoding/legacy-mb-korean | 435 | 43,246 / 240,148 | 239,942 / 240,148 |
| encoding/legacy-mb-schinese | 6 | 981 / 1,017 | 981 / 1,017 |
| encoding/legacy-mb-tchinese | 283 | 43,495 / 193,267 | 193,163 / 193,267 |
| encoding/streams | 24 | 164 / 219 | 164 / 219 |
| **Total** | **1,311** | **236,429 / 791,733** | **790,602 / 791,566** |

The candidate added 554,173 passing results and retained every earlier pass by URL, complete subtest name and multiplicity. Hundreds of character-level cases share each form fixture. This weighting matters when interpreting the overall browser percentage.

## MIME parsing and message transfer

The previous full-run replay left 275 formerly passing entries unresolved on 13 URLs. Every URL was run in reference–candidate–candidate–reference order. The candidate passed 268 of the 275 entries in both repetitions, including all 258 MIME entries. The WebSocket entry passed in both candidate trials and one reference trial. The other seven entries all used a single paint timing object.

A separate intermediate replay on `70260418` located a problem in the worker transfer path. Local cloning could move a MessagePort while the cross-realm serializer lost the reference in the message payload. The MessagePort test timed out, leaving a later validation of an invalid transfer list pending. Both dedicated and shared worker pages passed that validation twice on `e6850ee8`. The engine's unit coverage also sends a port and a buffer to a worker and back, checks identity against the event's port list and uses the returned endpoint to deliver a message.

## Paint observer registration

The IDL test installs an unbuffered paint observer after asynchronously loading IDL resources. RabbitHole records its two paint entries from a timer scheduled during load. When both entries already existed, the new observer received no future entry and its three-second timeout left the shared test object undefined.

A diagnostic copy of the fixture recorded the existing entries immediately after observer registration and added a buffered observer. Each engine exhibited both orders in its two diagnostic trials. When entries already existed, the buffered observer received first-paint and first-contentful-paint, and the original IDL object check failed. When registration preceded paint, the original check passed. This control explains how seven dependent checks can change together. It does not supply a browser paint timestamp from the renderer.

## WebAssembly fixture initialization

The WebAssembly Table JS constructor accepts numeric table limits and fixes the table's address type to 32 bits. The upstream default import registry also constructs a table with 64-bit addressing and BigInt limits. Conversion of those limits throws before the registry is installed. Subsequent modules can report missing imports or undefined exports even when their own instructions run correctly.

Two fixtures that do not use the 64-bit table were replayed against an isolated copy of the setup that omitted just that declaration. The ordinary reference had passed 314/336 alignment checks and 154/160 annotation checks. Both engines passed 336/336 and 160/160 respectively on both diagnostic repetitions. The finding identifies a missing JS API feature and demonstrates a shared prerequisite on these two pages. All published compatibility counts use the original fixture.

## Independent candidate validation

The full CSS pixel run on `e6850ee8` produced exactly the same outcome as the reference on all 19,627 paths. Both passed 16,398, using four workers, a 20-second timeout and mouse actions disabled. The normal and forced-JIT test262 runs at `2e58f87d8350ad51224617a041c6e041d52f8444` each passed 46,017 cases, failed 47 and skipped 7,614. Each used six workers and reported zero regressions against the committed test262 baseline. Neither comparison depends on the diagnostic fixture changes above.
