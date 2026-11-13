# Service workers: first executable milestone

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
already controlled document. An update waits while the previous worker controls
a client, unless the new script calls `skipWaiting()`.

The registry belongs to an `Engine` and survives document navigation within that
engine. It is currently memory-only: it does not survive process shutdown, share
registrations across independent engines, or wake a worker without a live engine
being ticked. Worker termination/restart and update checks for imported scripts
are future work. Only classic scripts are supported. Module scripts, messaging,
client enumeration, navigation preload, and fetch interception are not implemented;
unsupported exposed operations reject explicitly. Installing a worker therefore
does not yet provide offline navigation or intercept network requests.

Cache Storage currently retains its existing per-realm, in-memory implementation;
page and service-worker caches are not shared yet. Do not rely on a worker's
cache being visible to its controlled page.

Validation uses local HTTP fixtures in `crates/engine/tests/service_workers.rs`
and focused lifecycle tests in `crates/script/src/tests_service_worker.rs`.
The lifecycle and registration algorithms follow the primary
[W3C Service Workers specification](https://w3c.github.io/ServiceWorker/).
