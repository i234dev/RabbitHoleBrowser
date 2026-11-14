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
The bounded shader evaluator includes 32 typed float/vector math built-ins and
typed user helpers with input parameters, local values and a final return.
Nonrecursive calls have an accumulated evaluation budget; general shader control
flow remains open. Backend pixel tests, API tests and page-renderer fixtures
cover these milestones. See
[the current surface and limits](webgl.md).

An experimental WebGL2 context is now exposed with a distinct JavaScript
interface, bounded GLSL ES 3.00 source support, vertex array objects and checked
uint32 element indices with fixed primitive restart. Source language version
and context version are separate; WebGL1 retains its own semantics. These
features produce actual page pixels, but do not establish WebGL conformance or
Three.js compatibility.

The integrated milestones now implement WebGL2 instancing through VAO-captured
divisor attributes, and WebGL1/2 render-to-texture framebuffers with one RGBA8
color texture and optional DEPTH_COMPONENT16 storage. WebGL2 read/draw bindings
are independent; feedback loops and invalid attachments are checked before
pixels change. Joint regressions render distinct instances into an FBO, sample
the result onto the page and verify that the default canvas remains unchanged.
This remains a partial experimental implementation, not full WebGL2 conformance.

Next: broader shader grammar and rendering state, real integer shader types
(including `gl_InstanceID`), integer attributes/uniforms, mixed/minified texture
filtering and mipmaps, each with pixel regressions and workload validation.
Depth-only FBOs, color renderbuffers, multiple color attachments/MRT, stencil,
blits and multisampling remain open, alongside 3D textures, UBOs, queries,
synchronization and transform feedback. Exposed unsupported commands must keep
reporting errors until implemented.

### Service Worker lifecycle, memory cache and controlled navigation

Classic Service Worker scripts now execute in isolated worker realms, with
registration, install/activate lifetimes, rejected-install handling, updates,
waiting workers, `skipWaiting`, `clients.claim` and controller notifications.
Origin-scoped Cache API entries hold actual response bytes and shared storage
within an engine instance. This state survives document changes in that
instance, but does not survive engine shutdown or share state across independent
engines. Controlled documents' `window.fetch()` now dispatches `FetchEvent`,
supports asynchronous `respondWith()` responses and network fallback, and can
receive cached bytes without a network request. Worker-initiated fetches go
directly to the network, avoiding recursive interception. Initially controlled
top-level GET HTTP(S) navigation now also works in `Engine::load()` and the
asynchronous GUI, including cached HTML with a stopped server in the same Engine.
The committed page adopts the reserved client and retains its realm while
external scripts arrive. Headers, referrer filtering, redirect response policies
and cumulative cookie/Fetch Metadata state have local HTTP regression coverage.
Stop and superseding navigation propagate abort to `event.request.signal` and
discard stale delivery without replacing the old page before commit. See
[the lifecycle, cache and navigation scope](service-workers.md).

Next: interception for POST, frames and other resource loaders, then disk
persistence and restart tests. Initially uncontrolled navigation still uses the
existing loader, including redirects into a registered scope. Registrations and
Cache Storage remain memory-only within one Engine and are not shared across
independent tabs; the separate HTTP cache's disk storage does not persist Service
Worker state. Module Service Workers, messaging, client enumeration, navigation
preload and physical cancellation of existing HTTP transport also remain open.

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

## 6. Reduce MCP reading round trips

Implement the [MCP reading milestone](mcp-reading-plan.md) on the development
revision current at that time. Prioritise context around matches, multiple
search terms and `open` with an inline search, then lighter article reading
and summaries. Preserve refs, untrusted-content fences and existing defaults;
keep tool definitions within the recorded size budget. Verify gains with ten
RH-AGENT-001 runs per arm before publishing performance claims.
