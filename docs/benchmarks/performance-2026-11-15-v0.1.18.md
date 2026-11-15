# Performance and directed CSS checks — RabbitHole 0.1.18

Measured on **15 November 2026 (Europe/Madrid)**, using source
`dae023e7cc14d17501c18e39d6aafda2c4611902` for the performance runs.
These are measurements of **0.1.18** when shown by a later browser build;
they do not imply a rerun on 0.1.19. The
[portable JSON record](performance-2026-11-15-v0.1.18.json) preserves all 16
Speedometer scores, the eight paired ratios, all 27 microbenchmark results,
artifact/source hashes, conditions and the separately sourced CSS controls.

## Speedometer 3.1

| Statistic | Result |
|---|---:|
| Mean score, JIT off (8 runs) | 2.0820037774124547 |
| Mean score, JIT on (8 runs) | 2.1022003202961144 |
| Median of the 8 paired on/off score ratios | 1.006402226462936× |
| Range of paired ratios | 0.9903242718893842–1.036887061099561× |
| Pairs with a higher score using JIT | 5 / 8; acceptance required 6 / 8 |

The two score means are arithmetic means of eight complete runs per mode.
The paired statistic is calculated from each block’s on/off scores before
taking the median. They are different statistics; neither is a pooled
confidence interval or a statistically established global improvement.

**The consistency criterion was not met.** This result describes near-parity,
with no consistent global gain established. Phase 8 remains open and the JIT
remains off by default. No extra samples were added to change that decision.

Each run completed the official pinned Speedometer 3.1 suite:
**20 workloads, 58 subtests, three iterations and 199 exported metrics**.
The 16 timings used eight balanced blocks: off/on in odd blocks and on/off
in even blocks. A separate native-entry diagnostic was excluded from timing
and does not establish native coverage of every timed workload.

Conditions: release builds with the `jit` feature, Rust 1.99.1 / LLVM 23.1.1,
Linux x86-64 under WSL2 on an Intel Core i7-12700H, child pinned to CPU 0.
The project held a quiet window; Windows desktop/background idleness was not
claimed. Timed runs used normal GC, the default JIT threshold and
`RABBITHOLE_JIT=off` or `on`; no threshold or GC-stress override was set.
The complete timing interval was 2026-11-15 21:46:48–22:06:52 UTC
(22:46:48–23:06:52 Europe/Madrid).

Browser durations use `performance.now()`, backed by `std::time::Instant`.
External monotonic/boottime checks were valid. Recorded realtime adjustments
were retained and were not substituted for monotonic durations.

## Individual JavaScript microbenchmarks

The same 0.1.18 source completed **432 observations: 27 cases × eight paired
blocks × two modes**. Each observation ran one `perf -e` evaluation in a fresh
process on CPU 0. Evaluation time uses `CLOCK_THREAD_CPUTIME_ID`, rather than
process startup or wall time. The result below is **off time / on time**:
a ratio above 1 means higher throughput with JIT on. No overall speedup is
aggregated across unlike programs.

| Case | Median on/off speed ratio | Range over 8 pairs |
|---|---:|---:|
| `empty_loop` | 2.109× | 2.084–2.114× |
| `global_loop` | 2.267× | 2.128–2.336× |
| `local_loop` | 1.701× | 1.668–1.736× |
| `let_loop` | 1.991× | 1.929–2.084× |
| `push_objects` | 1.011× | 0.988–1.042× |
| `calls` | 1.206× | 1.092–1.271× |
| `closure_calls` | 1.531× | 1.509–1.591× |
| `method_calls` | 1.516× | 1.418–1.602× |
| `new_objects` | 1.233× | 1.158–1.264× |
| `props` | 2.946× | 2.778–3.021× |
| `prop_set` | 2.390× | 2.287–2.427× |
| `array_rw` | 1.987× | 1.949–2.013× |
| `strings` | 1.073× | 1.004–1.116× |
| `string_ops` | 1.057× | 1.030–1.108× |
| `array_map` | **0.990×** | 0.951–1.022× |
| `fib` | 1.172× | 1.149–1.207× |
| `json` | 1.009× | 0.989–1.018× |
| `for_in` | 1.091× | 1.043–1.111× |
| `try_catch` | 1.791× | 1.704–1.870× |
| `arguments` | 1.128× | 1.109–1.173× |
| `switch` | 2.560× | 2.464–2.635× |
| `hot_numeric` | 1.309× | 1.279–1.314× |
| `hot_7ops` | 2.165× | 2.131–2.206× |
| `hot_bits` | 1.760× | 1.727–1.768× |
| `hot_array` | 1.651× | 1.622–1.885× |
| `hot_props` | 1.376× | 1.351–1.508× |
| `hot_branchy` | 1.913× | 1.881–2.028× |

All cases passed the recorded micro gate; that does not mean that every case
sped up. In particular, `array_map` has median **0.9895722724206786×**
(rounded to 0.990×), with five of eight paired ratios below 1. The separate
11,072 semantic comparisons are correctness checks, not timing observations
or a full test262 rerun.

## Directed CSS controls

**33 / 33 selected pixel controls passed** on 15 November 2026:
31 WPT cases and two local controls. This was a profile-`ci` bootstrap run on
`37cb93d454c91e805210d350bad5dcc2bad88613`, not a run of the final release
binary. Its CSS, style, layout and graphics component Git trees are identical
to those in final 0.1.18; their hashes and all case names are in the JSON.
The oracles were unchanged. This selected result supplies **no fresh global
CSS/WPT percentage** and does not replace the committed full-corpus baseline.

## Reproducing the conditions

Check out the exact measured source above. Build the `perf` and `jitdiff`
examples from `rabbithole-js`, and `shot` from `rabbithole-engine`, with
`--profile release --features jit --locked`. Their measured binary hashes and
build commands are in `measurement_artifacts` in the JSON.

For micros, take the exact named program from `CASES` in
[`crates/js/examples/perf.rs`](https://github.com/i234dev/RabbitHoleBrowser/blob/dae023e7cc14d17501c18e39d6aafda2c4611902/crates/js/examples/perf.rs) at the measured
revision and launch a fresh `perf -e PROGRAM` process with `taskset -c 0` for
each observation. Collect all 27 cases in each of eight balanced off/on
blocks; retain each pair and its range instead of the example’s default
best-of-three total.

For Speedometer, serve the unmodified static
[pinned official suite](https://github.com/WebKit/Speedometer/tree/1386415be8fef2f6b6bbdbe1828872471c5d802a)
on loopback. Use `shot` at 1280 × 900 with
`/?iterationCount=3&startAutomatically`, waiting until the suite’s summary
section is visible and exporting `benchmarkClient.metrics`. Validate all
20 workloads, 58 subtests, three iterations and 199 metrics before accepting
a run. Repeat eight balanced pairs under the stated CPU/GC/JIT controls;
keep the native-entry diagnostic separate from the 16 timings.

The complete raw summaries, logs, PNGs and process/resource records remain
in the private evidence archive. Their root-reviewed summary hashes are
published in the JSON without private paths or environment contents.

Historical [0.1.11](speedometer-2026-11-14-v0.1.11.md) and
[0.1.7](speedometer-2026-11-14.md) records remain unchanged. These records do
not establish a causal performance change between versions, a same-machine
Chrome/V8 comparison, or Windows/ARM timing results. JS, CSS, DOM and other
full-corpus compatibility measurements retain their original dates.
