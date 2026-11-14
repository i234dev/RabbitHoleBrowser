# Service workers: lifecycle, memory cache and controlled navigation

RabbitHole runs classic service-worker scripts in separate JavaScript worker
realms. Registration fetches and evaluates the script before resolving; it does
not manufacture an active worker. `install` and `activate` dispatch real
`ExtendableEvent` objects, and `waitUntil()` delays lifecycle transitions.
Rejected installation promises make the worker redundant. `clients.claim()` and
`skipWaiting()` participate in controller selection and updates.

Registration requires a potentially trustworthy HTTP(S) client URL, same-origin
script and scope, a JavaScript MIME type, and an allowed scope. Redirected main
scripts and encoded path separators are rejected. `Service-Worker-Allowed` can
extend the script directory's default scope. HTTPS and HTTP loopback fixtures
are supported; secure-context ancestry is not modeled independently of the
client URL yet.

`ready`, `controller`, `getRegistration()`, `getRegistrations()`, `update()`, and
`unregister()` reflect the live registry. A new document can acquire a matching
active controller. Unregistering prevents future matching without clearing an
already controlled document. The worker becomes redundant once no client uses
the registration and lifecycle promises have settled. An update waits while the previous worker controls
a client, unless the new script calls `skipWaiting()`.

The registry belongs to an `Engine` and survives document navigation within that
engine. It is currently memory-only: it does not survive process shutdown, share
registrations across independent engines, or wake a worker without a live engine
being ticked. Worker termination/restart and update checks for imported scripts
are future work. Only classic scripts are supported. Module scripts, messaging,
client enumeration and navigation preload are unfinished; unsupported exposed
operations reject explicitly.

## Controlled-document fetch interception

`window.fetch()` from a controlled HTTP(S) document dispatches a real
`FetchEvent` in its active service worker. `respondWith()` must be called once,
synchronously during dispatch, and stops later listeners from handling that
event. Its promise can settle asynchronously with a `Response`, including one
returned by `caches.match()`. Status, status text, headers, response type and body
bytes cross between realms as owned data; JavaScript objects are not shared.

A request without `respondWith()` follows the existing network path. A rejected
promise, an invalid/used response, or canceling the event without a response
produces a fetch error rather than silently falling back to the network. The
client fetch promise rejects with `TypeError`; `FetchEvent.handled` reports a
failed service-worker response with `NetworkError`. `fetch(event.request)` inside
the worker goes to the network without recursively intercepting itself.

`waitUntil()` extends the event lifetime. Pending fetch events can finish in the
old runtime after an update or unregister, even when that worker has become
redundant. Client disposal releases its pending deliveries. Aborting a client
fetch preserves the caller's abort reason and drops its pending delivery, but
does not physically cancel the inherited HTTP transport. The abort reaches the
worker's `event.request.signal`, including request clones and delayed forwarding.
Canceling delivery does not cancel independent `waitUntil()` work: the old worker
can complete it after replacement, unregister and disposal of its client.

Request mode, credentials and redirect metadata are preserved when constructing
or cloning a request. Normal CORS requests retain the network layer's checks.
The current subset rejects `same-origin` requests to another origin, and
`no-cors` methods/headers/content types outside the safelist, before dispatching
or sending them. This is a bounded Fetch implementation; inherited network
response metadata and header guards still have gaps.

## Initially controlled navigation

Top-level, bodyless GET HTTP(S) navigation uses the active matching registration
in both `Engine::load()` and the GUI's asynchronous loading path. The worker runs
an actual navigation `FetchEvent` before the document is loaded. Its
`resultingClientId` identifies a reserved client that the committed document
adopts, including documents without scripts. Deferred external scripts reuse the
committed document's realm and client rather than creating another one.

A worker can return cached HTML bytes through `respondWith(caches.match(...))`.
After registration, activation and precaching, a later navigation can therefore
load that HTML with its server stopped, within the same Engine. A new Engine or
a separate tab does not inherit the registry or Cache Storage. This does not
provide persistent offline application startup.

An unhandled event falls back to the network; a rejected or invalid response
fails the navigation. Worker `fetch(event.request)` bypasses interception,
including clones and empty-init request copies. Live headers are preserved with
normal forbidden-header filtering. The internal navigation capability cannot be
acquired by editing a public request's `mode`. A nonempty request initialization
uses the worker's client and origin for the new request.

HTTP and synthetic redirects reselect the matching scope and controller at each
hop, with a bounded chain and a navigation deadline. The response's
`Referrer-Policy` applies before the next worker sees the request. Clones,
request copies and later weaker policies cannot recover referrer data already
discarded. Origin-only initiator metadata remains separate for CORS, Fetch
Metadata and cookies; cross-site redirect taint remains cumulative when a chain
returns to its starting site. A cached response's URL cannot change the
navigation's document origin.

The GUI pumps worker work between content-thread messages and sends only owned
response bytes and metadata to its background loader. Stop, replacement
navigation and tab disposal cancel pending delivery and propagate the request
signal. Generation checks discard stale loader results, while the previous page
and client remain until a new page commits. Already launched HTTP/resource loads
are not physically interrupted. Worker callbacks retain the existing JavaScript
watchdog and can occupy the content thread; the UI thread stays independent.
Intercepted responses and downloads are buffered; network reads retain the
fetcher's body limit. Synthetic responses and Cache Storage have no separate
quota enforcement yet.

Navigation interception starts only when the initial URL has an active matching
registration. An initially uncontrolled navigation keeps the existing loader,
including a redirect into a registered scope. POST, frames, generic
image/style/script loaders, XHR and dedicated/shared-worker fetches are outside
this scope. Opaque navigation responses and opaque redirects are rejected.
Unresolved explicit event lifetime promises do not yet have a user-agent lifetime
cap. Module workers, disk persistence and broader offline behavior remain open.

## Cache Storage

Cache Storage shares response bytes and request metadata by origin within the
Engine, including between documents and service-worker realms. Each match creates
a fresh response body. Cache handles remain usable after their name is deleted;
opening that name again creates a new cache. Request matching supports `Vary`,
`ignoreVary`, `ignoreSearch`, and `ignoreMethod`. Invalid writes (non-GET, non-HTTP,
partial responses, and `Vary: *`) reject without changing stored entries.

Cache Storage is also memory-only, without disk persistence, quota accounting,
eviction, or storage partitioning beyond origin. Deleted cache handles retain
their byte storage until the Engine is dropped. HTTP cache policy remains
separate from the explicit Cache API.

Validation uses local HTTP fixtures in `crates/engine/tests/service_workers.rs`,
the actual `ContentHandle` protocol in
`app/src/gui/content/tests/service_workers.rs`, focused script tests in
`crates/script/src/tests_service_worker.rs`, and network cases in
`crates/net/tests/loopback.rs`. They cover lifecycle ordering, cached HTML with a
stopped server, real client identity, delayed-script realm reuse, tab isolation,
headers, fallback without recursion, response errors, abort and replacement,
referrer privacy and cumulative redirect/cookie state. Network-polling coverage
includes a window with frequent timers. This is a bounded subset of the primary
[W3C Service Workers specification](https://w3c.github.io/ServiceWorker/) and
[WHATWG Fetch algorithms](https://fetch.spec.whatwg.org/).
