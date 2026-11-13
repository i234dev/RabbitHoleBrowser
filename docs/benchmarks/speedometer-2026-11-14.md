# Speedometer 3.1 measurement — 2026-11-14

RabbitHole 0.1.7 completed the official default workloads with a displayed score
of **2.19 ± 0.065**. This measurement used the release `shot` headless harness,
three iterations, JIT off and CPU 0 on an Intel Core i7-12700H running x86-64
Linux under WSL2. The suite reported its 95% confidence interval.

There were 20 workloads and 58 substeps per iteration, giving 174 measured
substeps across three iterations. This is a configured three-iteration result.
No project builds or other project benchmarks ran concurrently; host background
load was not otherwise controlled. A same-machine Chrome comparison has not
been measured. The previous undated 0.148 record used a different, loaded
three-core VM, so it does not establish a comparable performance change.

## Results and provenance

| Item | Recorded value |
|---|---|
| Start, UTC | 2026-11-13 23:31:17.402592 UTC |
| End, UTC | 2026-11-13 23:32:24.261525 UTC |
| Start, Europe/Madrid | 2026-11-14 00:31:17.402592 CET |
| End, Europe/Madrid | 2026-11-14 00:32:24.261525 CET |
| Score mean, unrounded | 2.185183143934601 |
| Score 95% interval half-width | 0.0645783367227167 |
| Score per iteration | 2.198150330049423; 2.1552539650722706; 2.2021451366821094 |
| Geomean mean | 457.6710195198779 ms |
| Geomean interval half-width | 13.616568853395876 ms |
| Geomean per iteration | 454.9279393359398; 463.98244300015364; 454.10267622354036 ms |
| External elapsed time, monotonic | 69.60036440599652 s |
| Child process CPU time | 65.979838 s |
| Viewport | 1280 × 900 |
| Rust compiler | 1.99.1 (`58cf8f9c3`, 2026-10-13) |
| Browser source revision | `d131916064c2fb55035b11f5098aa6b8869f4807` |
| `shot.rs` source SHA-256 | `fd6d10ad5590e1b4e7570beffa7cb17bedbb2c6beb1214952828598096de213e` |
| Executed `shot` binary SHA-256 | `0077721440dd76d0570cf72ed6db603ef7ac641459a57d4586d8b23e51c99f2d` |
| Official suite revision | `1386415be8fef2f6b6bbdbe1828872471c5d802a` (`release/3.1`) |
| Official suite archive SHA-256 | `cfefa818d7789ed2f2b9f3f1bf608cc35e4e8241489eef47049296ce92791313` |

The source is the [official WebKit/Speedometer repository at the pinned
revision](https://github.com/WebKit/Speedometer/tree/1386415be8fef2f6b6bbdbe1828872471c5d802a).
All 1,118 files matched their upstream Git blob hashes and the exact tree file
set. The prebuilt workloads were served unchanged over loopback, preserving
the upstream BSD two-clause license and bundled notices. No package installation
or workload rebuild was needed. The release branch and benchmark UI identify
version 3.1; upstream `package.json` retains older development version metadata.

## Reproduction

Use a separate checkout of the browser source revision above and an unmodified
checkout of the pinned suite revision. Set `suite_dir` to that suite checkout,
then start a local static server:

```sh
python3 -m http.server 8765 --bind localhost --directory "$suite_dir"
```

From the browser checkout, build the harness and run it in a quiet window:

```sh
export CARGO_TARGET_DIR=target
cargo build --release --locked --features jit -p rabbithole-engine --example shot -j 2
export RABBITHOLE_JIT=off
export RH_JS_WAIT=1800000
export RH_PROGRESS='document.getElementById("info-progress").textContent'
export RH_DONE='document.documentElement.dataset.visibleSection === "summary"'
export RH_EVAL='JSON.stringify({section:document.documentElement.dataset.visibleSection,score:document.getElementById("result-number").textContent,confidence:document.getElementById("confidence-number").textContent,suiteNames:Suites.filter(s=>!s.disabled).map(s=>s.name),metrics:benchmarkClient.metrics})'
timeout --signal=TERM --kill-after=10s 2100s taskset -c 0 \
  ./target/release/examples/shot \
  'http://localhost:8765/?iterationCount=3&startAutomatically' \
  summary.png 1280 900 > speedometer.log 2>&1
```

Start from an environment without other `RH_*` or `RABBITHOLE_JIT*` overrides.
The measurement wrapper sampled `CLOCK_REALTIME`, `CLOCK_MONOTONIC`,
`CLOCK_BOOTTIME` and its process CPU clock every second, and recorded child
user/system CPU time before and after execution. Keep equivalent external
clock observations with the console output, final metric JSON and screenshot.
The harness prints the final JSON string between single quotation marks;
remove those outer display delimiters before parsing the JSON.

## Completion and clock checks

The process exited successfully, the screenshot showed the result summary and
the console contained no reported errors. The 199 metrics contained finite,
nonnegative samples; each measured substep had three samples. Independent
checks of sync-plus-async totals, workload totals, iteration geometric means
and scores matched the exported data.

`CLOCK_REALTIME` adjusted backwards by approximately 1.350 s and 1.391 s during
the run. The 70 external clock samples showed continuous monotonic and boottime
clocks: sampling intervals were 1.00024–1.05256 s, and differences between those
two clocks' intervals stayed below 7.1 microseconds. This also explains why the
UTC endpoints differ from the monotonic elapsed time.

The suite measures durations with `performance.now()` in
`resources/benchmark-runner.mjs`. RabbitHole implements that using
`Engine.start.elapsed()` from a `std::time::Instant` origin
([engine clock](../../crates/engine/src/scripting.rs),
[performance binding](../../crates/script/src/window.rs)). The realtime
adjustments therefore did not affect the stable clock used for this score.
The screenshot, exported values and clock evidence were reviewed before this
measurement was accepted.
