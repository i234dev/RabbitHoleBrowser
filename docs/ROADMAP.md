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
support `readPixels`. Depth, uniforms and RGBA8 nearest 2D textures are covered
by backend pixel tests, API tests and page-renderer tests. See
[the current surface and limits](webgl.md).

Next: expand GLSL and rendering state with pixel regressions, then validate
real workloads against the supported surface. Framebuffer objects, blending,
general shader control flow and linear/mipmapped textures remain open.
WebGL2 still returns `null`. The backend's optional GLSL ES 3.00 parsing mode
is preparatory work; a WebGL2 milestone also needs coherent context state,
objects and APIs, including vertex array objects, before it can be exposed.
Modern Three.js compatibility has not been established.

### Service Worker lifecycle and memory cache

Classic Service Worker scripts now execute in isolated worker realms, with
registration, install/activate lifetimes, rejected-install handling, updates,
waiting workers, `skipWaiting`, `clients.claim` and controller notifications.
Origin-scoped Cache API entries hold actual response bytes and shared storage
within an engine instance. This state survives document changes in that
instance, but does not survive engine shutdown or share state across independent
engines. See [the lifecycle and cache scope](service-workers.md).

Next: implement fetch-event interception and `respondWith`, validate controlled
navigation and offline behavior, then add disk persistence and restart tests.
Module Service Workers, messaging, client enumeration and navigation preload
also remain open. Lifecycle and cache tests do not establish offline support;
the separate HTTP cache's disk storage does not persist Service Worker state.

## 5. Validate usability and deliver builds

Use the fixed smoke-test list in `scripts/site-smoke.sh` and inspect rendered
screenshots. Test navigation, links, forms, scrolling, media and session restore
in the desktop browser. DOM/text counts alone do not verify these interactions.
Track regressions separately from network errors and external bot challenges.

Publish platform builds from the tested revision and record which revision
each executable contains. Expand macOS/Linux packaging and Windows execution
coverage alongside compatibility work.
