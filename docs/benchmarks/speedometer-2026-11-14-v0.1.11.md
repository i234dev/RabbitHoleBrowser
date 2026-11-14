# Speedometer 3.1 measurement — RabbitHole 0.1.11

The headless release `shot` harness on source
`baca2a7bda017e3690adbe118e6cae8ab2486ceb` completed the 20 default workloads
with a displayed score of **2.37 ± 0.094**. It used three iterations, JIT off
and CPU 0 on an Intel Core i7-12700H running x86-64 Linux under WSL2.
The [machine-readable record](speedometer-2026-11-14-v0.1.11.json) preserves
the numeric score, geometric means and evidence hashes.

There were 58 measured substeps per iteration, giving 174 substep samples
across three iterations and 199 exported aggregate metrics. This is the
configured three-iteration scope. Project build/conformance work was excluded
on the benchmark host while the project locks were held. Windows desktop and
other host background idleness were not asserted. A test262 run on a separate
Linux VM did not share this benchmark host.

## Results and provenance

| Item | Recorded value |
|---|---|
| Start, UTC | 2026-11-14 15:55:38.095444 UTC |
| End, UTC | 2026-11-14 15:56:43.412788 UTC |
| Start, Europe/Madrid | 2026-11-14 16:55:38.095444 CET |
| End, Europe/Madrid | 2026-11-14 16:56:43.412788 CET |
| Score mean, unrounded | 2.3684989807849735 |
| Score interval half-width | 0.09412541346152012 |
| Score per iteration | 2.3656591154908138; 2.4077295876718297; 2.3321082391922774 |
| Geomean mean | 422.28025618020274 ms |
| Geomean interval half-width | 16.75374956123651 ms |
| Geomean per iteration | 422.7151720430885; 415.3290324296578; 428.7965640678619 ms |
| External monotonic clock sample span | 64.309508487 s |
| Child process CPU time | 61.503516 s user + 2.532027 s system |
| Viewport | 1280 × 900 |
| Rust compiler | 1.99.1 (`58cf8f9c3`, 2026-10-13) |
| Build profile | release |
| Executed `shot` binary SHA-256 | `6ac2f66794099f2ba6532111c171aa00e1976bfb77e7d07e427820e06f92a473` |
| Official suite revision | `1386415be8fef2f6b6bbdbe1828872471c5d802a` (`release/3.1`) |
| Corpus file manifest SHA-256 | `089c45497bd05baabf4aca89d5af46a2f20550dfa1163adc20bea1e80ccdde05` |

The corpus is the [official pinned Speedometer source](https://github.com/WebKit/Speedometer/tree/1386415be8fef2f6b6bbdbe1828872471c5d802a).
All 1,118 files and the corpus file set were verified before and after execution.
The prebuilt workloads were served unchanged over loopback, retaining the
upstream BSD two-clause license and bundled notices. The source, binary and
measurement tools also retained their pinned hashes.

## Reproduction

Use a separate checkout of the browser revision above and the unmodified pinned
suite. Set `suite_dir` to the suite checkout and start a loopback static server:

```sh
python3 -m http.server 8765 --bind localhost --directory "$suite_dir"
```

Build the harness, then use a quiet project window with the same workload,
CPU affinity and environment. These are reproduction paths rather than the
private evidence directory used for the recorded run:

```sh
cargo build --profile release --features jit --locked -p rabbithole-engine --example shot
export LANG=C LC_ALL=C RABBITHOLE_JIT=off
export RH_JS_WAIT=1800000
export RH_PROGRESS='document.getElementById("info-progress").textContent'
export RH_DONE='document.documentElement.dataset.visibleSection === "summary"'
export RH_EVAL='JSON.stringify({section:document.documentElement.dataset.visibleSection,score:document.getElementById("result-number").textContent,confidence:document.getElementById("confidence-number").textContent,suiteNames:Suites.filter(s=>!s.disabled).map(s=>s.name),metrics:benchmarkClient.metrics})'
timeout --signal=TERM --kill-after=10s 300s taskset -c 0 \
  ./target/release/examples/shot \
  'http://localhost:8765/?iterationCount=3&startAutomatically' \
  summary.png 1280 900 > speedometer.log 2>&1
```

Start without other `RH_*` or `RABBITHOLE_*` overrides. Retain the console log,
final metric JSON, screenshot, binary/source/corpus hashes, child exit status
and external realtime, monotonic, boottime and process-CPU clock samples.
The harness displays the final JSON string between single quotation marks;
remove those display delimiters before parsing the JSON.

## Completion and clock checks

The process exited 0 without timeout or cancellation and its owned child was
reaped. The screenshot showed the summary and score. All 199 metrics contained
finite values; substep sample counts, totals, geometric means, scores and
confidence were independently recomputed. The source and corpus were unchanged.

The 66 external clock samples passed the clock checks, with no anomalies or
realtime adjustments recorded. The monotonic and boottime spans were
64.309508487 s and 64.309509101 s. Speedometer durations use
`performance.now()`, backed by RabbitHole's `std::time::Instant` origin
([engine clock](../../crates/engine/src/scripting.rs),
[performance binding](../../crates/script/src/window.rs)). The coordinator
reviewed the numeric export, clock evidence, source/corpus checks and screenshot
before accepting this measured scope.

The [0.1.7 record](speedometer-2026-11-14.md) remains historical. These records
do not establish a causal speed change between versions, default-iteration or
interactive-GUI performance, a same-machine Chrome/V8 comparison, or execution
of a later packaging revision.
