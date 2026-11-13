# Development plan

RabbitHole's goal is a usable desktop browser with its own HTML, CSS,
JavaScript, WebAssembly, layout and rendering engines. Platform integration,
TLS, font shaping and media codecs currently use the dependencies listed in
the README. Compatibility percentages measure specific test corpora; they
do not establish that every real site works.

## 1. Recovered work validated

- DOM: streaming HTML parsing, `document.write`, DOM parsing/serialization,
  scrolling and form-associated custom elements were recovered. WPT comparison
  against the original corpus exposed iframe, worker, messaging and Blob
  regressions; these were fixed before refreshing the baseline.
- JavaScript: the conformance fixes, optional-chain deletion, callback
  improvements and checked lazy parser were recovered. test262 gained 80
  passes without regressions; checked lazy parsing remains opt-in.
- CSS: custom text-overflow markers and dynamic line-clamp regression coverage
  were added. Combined validation also fixed repeated scroll translations
  when reusing cached layout. Continue with the remaining overflow and
  compositing cases.

Each delivery requires relevant regressions, comparison with the previous
baseline, workspace tests and an updated Windows build. A failing test must
not disappear merely because the baseline was regenerated.

The start page now gets its full version from Cargo and its current compatibility
counts from the committed baseline files. Measurement dates and workload scope
remain attached to the results; historical performance numbers must be replaced
only after a measured run. See [the measurement records](../tools/metrics/README.md).

## 2. Make the JavaScript JIT a measured improvement

The baseline x86-64 JIT, loop OSR, runtime helpers and incremental-GC barriers
are implemented. Native compilation is available with `--jit=on`, but stays
off by default. See [the design and implementation record](jit-design.md),
especially the Stage 2 sections; the initial feasibility study is historical.

Next steps, in order:

1. Reduce native call/return and resumption overhead, preserving the
   interpreter's frame semantics, exceptions, realm changes and watchdog.
2. Measure interpreter versus JIT on the same build, with alternating runs:
   `perf`, framework workloads and Speedometer. Report compilation time and
   code memory alongside execution time.
3. Investigate remaining overhead, including frame setup and executable-memory
   allocation, only when profiles show a useful target.
4. Enable by default only after compatibility and GC-stress checks pass,
   no microbenchmark falls below 0.95 times interpreter throughput, and
   Speedometer shows a repeatable improvement. Windows additionally needs
   execution tests on a real Windows machine; a cross-build alone is insufficient.

## 3. Complete parsing and improve real-page responsiveness

- Make lazy parsing report early errors before script execution. The checked
  parallel validator is an intermediate implementation; reduce its overhead
  before changing its default.
- Improve expensive DOM operations, calls and allocations using profiles from
  real pages. Keep scrolling and browser controls responsive during script work.
- Continue parser reentrancy and speculative loading, with tests covering
  parser-inserted scripts, asynchronous work and incremental layout.

## 4. Raise web-platform and rendering compatibility

- DOM priorities: CSSOM View, HTML/DOM parsing, custom elements, events,
  collections and file APIs, guided by reproducible WPT failures.
- CSS priorities: overflow, compositing, positioned inline containing blocks,
  masking, selectors/nesting, transforms, images and borders.
- Continue realm integration and the partial Service Worker and WebGL milestones
  below; neither implementation completes platform compatibility.
- Intl and Temporal remain deferred.
- An AArch64 emitter for the JavaScript and WebAssembly JITs is a separate
  backend project, to be validated on real ARM hardware including Apple Silicon.

### Experimental WebGL

The first WebGL1 rendering milestone is implemented: JavaScript shader/program
and buffer APIs drive our software backend, produce visible canvas pixels and
support `readPixels`. Depth, uniforms, independent RGB/alpha blending and RGBA8
nearest/bilinear 2D textures are implemented. Minification and magnification
filters must match; mipmaps and derivative-based LOD selection remain open.
The bounded shader evaluator includes 32 typed float/vector math built-ins,
but still lacks general control flow and user-defined functions. Backend pixel
tests, API tests and page-renderer fixtures cover these milestones. See
[the current surface and limits](webgl.md).

An experimental WebGL2 context is now exposed with a distinct JavaScript
interface, bounded GLSL ES 3.00 source support, vertex array objects and checked
uint32 element indices with fixed primitive restart. Source language version
and context version are separate; WebGL1 retains its own semantics. These
features produce actual page pixels, but do not establish WebGL conformance or
Three.js compatibility.

Next: framebuffer objects, instancing, broader shader grammar and rendering
state, mixed/minified texture filtering and mipmaps, each with pixel regressions
and workload validation. WebGL2 integer attributes/uniforms, 3D textures, UBOs,
queries, synchronization, transform feedback and MRT remain open. Exposed
unsupported commands must keep reporting errors until implemented.

### Service Worker lifecycle, memory cache and controlled fetch

Classic Service Worker scripts now execute in isolated worker realms, with
registration, install/activate lifetimes, rejected-install handling, updates,
waiting workers, `skipWaiting`, `clients.claim` and controller notifications.
Origin-scoped Cache API entries hold actual response bytes and shared storage
within an engine instance. This state survives document changes in that
instance, but does not survive engine shutdown or share state across independent
engines. Controlled documents' `window.fetch()` now dispatches `FetchEvent`,
supports asynchronous `respondWith()` responses and network fallback, and can
receive cached bytes without a network request. Worker-initiated fetches go
directly to the network, avoiding recursive interception. Client abort reasons
are preserved; propagation to `event.request.signal` remains open. See
[the lifecycle, cache and fetch scope](service-workers.md).

Next: controlled navigation and other resource loaders, worker request-signal
propagation and offline behavior, then disk persistence and restart tests.
Module Service Workers, messaging, client enumeration and navigation preload
also remain open. Existing lifecycle, cache and controlled-fetch tests do not
establish offline navigation; the separate HTTP cache's disk storage does not
persist Service Worker state.

## 5. Validate usability and deliver builds

Use the fixed smoke-test list in `scripts/site-smoke.sh` and inspect rendered
screenshots. Test navigation, links, forms, scrolling, media and session restore
in the desktop browser. DOM/text counts alone do not verify these interactions.
Track regressions separately from network errors and external bot challenges.

Google Account screens have reported layout and keyboard-interaction gaps.
The password caret's UTF-8 crash has a regression fix, but full authenticated
sign-in still needs interactive validation.

Publish platform builds from the tested revision and record which revision
each executable contains. Expand macOS/Linux packaging and Windows execution
coverage alongside compatibility work.
