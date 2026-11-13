# Development plan

RabbitHole's goal is a usable desktop browser with its own HTML, CSS,
JavaScript, WebAssembly, layout and rendering engines. Platform integration,
TLS, font shaping and media codecs currently use the dependencies listed in
the README. Compatibility percentages measure specific test corpora; they
do not establish that every real site works.

## 1. Complete and validate recovered work

- DOM: streaming HTML parsing, `document.write`, DOM parsing/serialization,
  scrolling and form-associated custom elements. Compare WPT against the
  committed corpus and investigate each regression before updating baselines.
- JavaScript: recover the conformance fixes and unfinished optional-chain
  deletion; preserve the callback improvements and early-error validation.
  Run test262 against its baseline, without expanding the deferred Intl work.
- CSS: fix dynamic removal of line clamping and implement custom text-overflow
  markers, then continue the remaining overflow and compositing cases.

Completion requires relevant regressions, comparison with the previous
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
- Implement the remaining realm integration and evaluate Service Workers and
  WebGL as separate projects with their own designs, tests and milestones.
- Intl and Temporal remain deferred.

## 5. Validate usability and deliver builds

Use the fixed smoke-test list in `scripts/site-smoke.sh` and inspect rendered
screenshots. Test navigation, links, forms, scrolling, media and session restore
in the desktop browser. DOM/text counts alone do not verify these interactions.
Track regressions separately from network errors and external bot challenges.

Publish platform builds from the tested revision and record which revision
each executable contains. Expand macOS/Linux packaging and Windows execution
coverage alongside compatibility work.
