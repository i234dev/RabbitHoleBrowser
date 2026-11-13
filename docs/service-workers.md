# Service workers: lifecycle, memory cache and controlled fetch

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
does not physically cancel the inherited HTTP transport. Propagating that abort
to the worker's `event.request.signal` is still pending.

Request mode, credentials and redirect metadata are preserved when constructing
or cloning a request. Normal CORS requests retain the network layer's checks.
The current subset rejects `same-origin` requests to another origin, and
`no-cors` methods/headers/content types outside the safelist, before dispatching
or sending them. This is a bounded Fetch implementation; inherited network
response metadata and header guards still have gaps.

Interception currently covers only controlled documents' JavaScript fetches.
Navigation, image/style/script loaders, XHR and dedicated/shared-worker fetches
are outside this milestone. Serving a cached response to `window.fetch()` does
not yet provide offline navigation or a persistent offline application.

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

Validation uses local HTTP fixtures in `crates/engine/tests/service_workers.rs`
and focused tests in `crates/script/src/tests_service_worker.rs`: lifecycle
ordering, cached responses without network requests, fallback, worker network
fetches without recursion, response errors, binary bodies, abort reasons,
credential modes and rejected unsafe requests. The implementation follows the
implemented subset of the primary
[W3C Service Workers specification](https://w3c.github.io/ServiceWorker/).
